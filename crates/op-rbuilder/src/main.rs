use std::future::Future;
use std::sync::Arc;
use clap::Parser;
use monitoring::Monitoring;
use reth::providers::CanonStateSubscriptions;
use reth::tasks::TaskExecutor;
use reth_optimism_cli::{chainspec::OpChainSpecParser, Cli};
use reth_optimism_node::node::OpAddOnsBuilder;
use reth_optimism_node::OpNode;

#[cfg(feature = "flashblocks")]
use payload_builder::CustomOpPayloadBuilder;
#[cfg(not(feature = "flashblocks"))]
use payload_builder_vanilla::CustomOpPayloadBuilder;
use reth_transaction_pool::TransactionPool;
use rundler::cli::{construct_providers, load_configs, CommonArgs};
use rundler::cli::pool::PoolArgs;
use primitives::rundler::{Event, is_nonspammy_event};
use rundler::cli::builder::{BuilderArgs};
use rundler::cli::rpc::RpcArgs;
use rundler_provider::Providers;
use rundler_builder::{BuilderEvent, BuilderEventKind, BuilderTask, LocalBuilderBuilder, LocalBuilderHandle};
use rundler_pool::{LocalPoolBuilder, LocalPoolHandle, PoolEvent, PoolTask};
use tokio::sync::broadcast;
use rundler_utils::emit::{self, WithEntryPoint, EVENT_CHANNEL_CAPACITY};
use rundler_rpc::{EthApi, RundlerApi, EntryPointRouterBuilder, EntryPointRouteImpl, UserOperationEventProviderV0_7, EntryPointRouter, RpcTaskArgs, EthApiServer, RundlerApiServer};
use rundler_sim::gas::{get_fee_oracle, FeeEstimatorImpl, FeeOracle};
use rundler_sim::{FeeEstimator, GasEstimatorV0_7};

/// CLI argument parsing.
pub mod args;
pub mod generator;
#[cfg(test)]
mod integration;
mod metrics;
mod monitor_tx_pool;
mod monitoring;
#[cfg(feature = "flashblocks")]
pub mod payload_builder;
#[cfg(not(feature = "flashblocks"))]
mod payload_builder_vanilla;
mod primitives;
#[cfg(test)]
mod tester;
mod tx_signer;
use monitor_tx_pool::monitor_tx_pool;

/// Const from rundler cli
const REQUEST_CHANNEL_CAPACITY: usize = 1024;
/// Const from rundler cli
const BLOCK_CHANNEL_CAPACITY: usize = 1024;

fn main() {
    Cli::<OpChainSpecParser, args::OpRbuilderArgs>::parse()
        .run(|builder, builder_args| async move {
            let rollup_args = builder_args.rollup_args.clone();

            let op_node = OpNode::new(rollup_args.clone());


            let pool_builder = LocalPoolBuilder::new(REQUEST_CHANNEL_CAPACITY, BLOCK_CHANNEL_CAPACITY);
            let pool_handle = pool_builder.get_handle();
            // Everything used for rpc task spawn
            let (providers, rpc_task_args, router, fee_estimator, pool_fut, builder_fut) = spawn_rundler(builder.task_executor(), builder_args.clone(), pool_builder).await;

            let handle = builder
                .with_types::<OpNode>()
                .with_components(op_node.components().payload(CustomOpPayloadBuilder::new(
                    builder_args.builder_signer,
                    builder_args.flashblocks_ws_url,
                    builder_args.chain_block_time,
                    builder_args.flashblock_block_time,
                )))
                .with_add_ons(
                    OpAddOnsBuilder::default()
                        .with_sequencer(rollup_args.sequencer_http.clone())
                        .with_enable_tx_conditional(rollup_args.enable_tx_conditional)
                        .build(),
                )
                .extend_rpc_modules(move |ctx| {
                    let eth_api = EthApi::new(rpc_task_args.chain_spec.clone(), router.clone(), pool_handle.clone(), rpc_task_args.eth_api_settings.permissions_enabled);
                    let rundler_api = RundlerApi::new(&rpc_task_args.chain_spec, router, pool_handle,fee_estimator,providers.evm().clone());
                    ctx.modules.merge_configured(eth_api.into_rpc())?;
                    ctx.modules.merge_configured(rundler_api.into_rpc())?;
                    Ok(())
                })
                .on_node_started(move |ctx| {
                    let new_canonical_blocks = ctx.provider().canonical_state_stream();
                    let builder_signer = builder_args.builder_signer;

                    if builder_args.log_pool_transactions {
                        tracing::info!("Logging pool transactions");
                        ctx.task_executor.spawn_critical(
                            "txlogging",
                            Box::pin(async move {
                                monitor_tx_pool(ctx.pool.all_transactions_event_listener()).await;
                            }),
                        );
                    }

                    ctx.task_executor.spawn_critical(
                        "monitoring",
                        Box::pin(async move {
                            let monitoring = Monitoring::new(builder_signer);
                            let _ = monitoring.run_with_stream(new_canonical_blocks).await;
                        }),
                    );

                    // Spawn rundler pool and builder futures
                    ctx.task_executor.spawn_critical(
                        "rundler pool",
                        pool_fut
                    );
                    ctx.task_executor.spawn_critical(
                        "rundler pool",
                        builder_fut
                    );
                    Ok(())
                })
                .launch()
                .await?;

            handle.node_exit_future.await
        })
        .unwrap();
}

/// Spawns rundler components and return everything we need to extend rpc methods
async fn spawn_rundler(
    task_executor: &TaskExecutor,
    builder_args: args::OpRbuilderArgs,
    pool_builder: LocalPoolBuilder
) -> (impl Providers + 'static, RpcTaskArgs, EntryPointRouter, impl FeeEstimator + 'static, core::pin::Pin<Box<impl Future<Output=()>>>, core::pin::Pin<Box<impl Future<Output=()>>>) {
    let pool_args = builder_args.pool;
    let common_args = builder_args.common;
    let rundler_builer_args = builder_args.builder;
    let rpc_args = builder_args.rundler_rpc;

    // hook pool and builder into rpc via ctx.extend_modules
    let task_executor = task_executor.clone();
    // Hardcode, figure out how to do it nicely
    let chain_spec = rundler::cli::chain_spec::resolve_chain_spec(&Some(String::from("optimism_sepolia")), &None);
    let providers = construct_providers(&common_args, &chain_spec).unwrap();
    let (mempool_configs, entry_point_builders) = load_configs(&common_args).await.unwrap();
    let pool_task_args = pool_args
        .to_args(
            chain_spec.clone(),
            &common_args,
            None,
            mempool_configs.clone(),
            entry_point_builders.clone(),
        )
        .await.expect("build pool args");
    let builder_task_args = rundler_builer_args
        .to_args(
            chain_spec.clone(),
            &common_args,
            None,
            mempool_configs,
            entry_point_builders,
        )
        .await.expect("build builder args");
    let rpc_task_args = rpc_args.to_args(chain_spec.clone(), &common_args).expect("build rpc args");
    let (event_sender, event_rx) =
        broadcast::channel::<WithEntryPoint<Event>>(EVENT_CHANNEL_CAPACITY);
    let (op_pool_event_sender, op_pool_event_rx) =
        broadcast::channel::<WithEntryPoint<PoolEvent>>(EVENT_CHANNEL_CAPACITY);
    let (builder_event_sender, builder_event_rx) =
        broadcast::channel::<WithEntryPoint<BuilderEvent>>(EVENT_CHANNEL_CAPACITY);
    // Spawn rundler event logger
    task_executor.spawn_critical(
        "recv and log events",
        Box::pin(emit::receive_and_log_events_with_filter(event_rx, |_| true)),
    );
    // Router Pool event -> logger
    task_executor.spawn_critical(
        "recv op pool events",
        Box::pin(emit::receive_events("op pool", op_pool_event_rx, {
            let event_sender = event_sender.clone();
            move |event| {
                let _ = event_sender.send(WithEntryPoint::of(event));
            }
        })),
    );
    // Router Builder event -> logger
    task_executor.spawn_critical(
        "recv builder events",
        Box::pin(emit::receive_events("builder", builder_event_rx, {
            let event_sender = event_sender.clone();
            move |event| {
                if is_nonspammy_event(&event) {
                    let _ = event_sender.send(WithEntryPoint::of(event));
                }
            }
        })),
    );


    let pool_providers = providers.clone();
    let pool_handle = pool_builder.get_handle();
    let pool_task_executor = task_executor.clone();
    // Spawn pool
    let pool = Box::pin(async move {
        PoolTask::new(
            pool_task_args,
            op_pool_event_sender,
            pool_builder,
            pool_providers,
        ).spawn(pool_task_executor).await.unwrap();
    });



    let builder_providers = providers.clone();
    let builder_task_executor = task_executor.clone();
    let builder_chain_spec = chain_spec.clone();
    // Spawn builder
    let builder = Box::pin(async move {
        let signer_manager = rundler_signer::new_signer_manager(
            &builder_task_args.signing_scheme,
            builder_task_args.auto_fund,
            &builder_chain_spec,
            builder_providers.evm().clone(),
            builder_providers.da_gas_oracle().clone(),
            &builder_task_executor,
        )
            .await.expect("create signer manager");

        let builder_builder = LocalBuilderBuilder::new(
            REQUEST_CHANNEL_CAPACITY,
            signer_manager.clone(),
            Arc::new(pool_handle.clone()),
        );
        BuilderTask::new(
            builder_task_args,
            builder_event_sender,
            builder_builder,
            pool_handle.clone(),
            builder_providers,
            signer_manager,
        )
            .spawn(builder_task_executor).await.unwrap()
    });

    // We construct additional components that would be used by rpc server
    let fee_oracle = Arc::<dyn FeeOracle>::from(get_fee_oracle(
        &chain_spec,
        providers.evm().clone(),
    ));
    let fee_estimator = FeeEstimatorImpl::new(
        providers.evm().clone(),
        fee_oracle,
        rpc_task_args.precheck_settings.priority_fee_mode,
        rpc_task_args.precheck_settings.bundle_base_fee_overhead_percent,
        rpc_task_args
            .precheck_settings
            .bundle_priority_fee_overhead_percent,
    );

    // We build only v7 entrypoint, don't server v6 for now
    // TODO: figure out if we need to support v6

    let ep = providers
        .ep_v0_7()
        .clone()
        .expect("entry point v0.7 not supplied");
    let router_builder = EntryPointRouterBuilder::default().v0_7(EntryPointRouteImpl::new(
        ep.clone(),
        GasEstimatorV0_7::new(
            chain_spec.clone(),
            providers.evm().clone(),
            ep.clone(),
            rpc_task_args.estimation_settings,
            fee_estimator.clone(),
        ),
        UserOperationEventProviderV0_7::new(
            chain_spec.clone(),
            providers.evm().clone(),
            rpc_task_args
                .eth_api_settings
                .user_operation_event_block_distance,
            rpc_task_args
                .eth_api_settings
                .user_operation_event_block_distance_fallback,
        ),
    ));
    let router = router_builder.build();
    (providers, rpc_task_args, router, fee_estimator, pool, builder)
}

