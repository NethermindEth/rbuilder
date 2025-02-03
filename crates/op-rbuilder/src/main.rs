use clap::Parser;
use generator::CustomOpPayloadBuilder;
use monitoring::Monitoring;
use payload_builder_vanilla::OpPayloadBuilderVanilla;
use reth::{
    builder::{
        components::PayloadServiceBuilder, engine_tree_config::TreeConfig, node::FullNodeTypes,
        BuilderContext, EngineNodeLauncher, Node,
    },
    payload::PayloadBuilderHandle,
    providers::{providers::BlockchainProvider, CanonStateSubscriptions},
    transaction_pool::TransactionPool,
};
use reth_basic_payload_builder::BasicPayloadJobGeneratorConfig;
use reth_node_api::{NodeTypesWithEngine, TxTy};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_cli::{chainspec::OpChainSpecParser, Cli};
use reth_optimism_evm::OpEvmConfig;
use reth_optimism_node::{OpEngineTypes, OpNode};
use reth_payload_builder::PayloadBuilderService;
use tx_signer::Signer;

/// CLI argument parsing.
pub mod args;

pub mod generator;

mod metrics;
mod monitoring;
pub mod payload_builder;
mod payload_builder_vanilla;
#[cfg(test)]
mod tester;
mod tx_signer;

fn main() {
    Cli::<OpChainSpecParser, args::OpRbuilderArgs>::parse()
        .run(|builder, builder_args| async move {
            let rollup_args = builder_args.rollup_args;

            let vanilla_builder = OpPayloadBuilderVanilla::new(
                OpEvmConfig::new(builder.config().chain.clone()),
                builder_args.builder_signer,
            );

            let engine_tree_config = TreeConfig::default()
                .with_persistence_threshold(builder_args.engine.persistence_threshold)
                .with_memory_block_buffer_target(builder_args.engine.memory_block_buffer_target);

            let op_node = OpNode::new(builder_args.rollup_args);
            let handle = builder
                .with_types_and_provider::<OpNode, BlockchainProvider<_>>()
                .with_components(
                    op_node
                        .components()
                        .payload(CustomOpPayloadBuilder::new(vanilla_builder)),
                )
                .with_add_ons(op_node.add_ons())
                /*
                // TODO: ExEx in Op-reth fails from time to time when restarting the node.
                // Switching to the spawn task on the meantime.
                // https://github.com/paradigmxyz/reth/issues/14360
                .install_exex("monitoring", move |ctx| {
                    let builder_signer = builder_args.builder_signer;
                    if let Some(signer) = &builder_signer {
                        tracing::info!("Builder signer address is set to: {:?}", signer.address);
                    } else {
                        tracing::info!("Builder signer is not set");
                    }
                    async move { Ok(Monitoring::new(builder_signer).run_with_exex(ctx)) }
                })
                */
                .on_node_started(move |ctx| {
                    let new_canonical_blocks = ctx.provider().canonical_state_stream();
                    let builder_signer = builder_args.builder_signer;

                    ctx.task_executor.spawn_critical(
                        "monitoring",
                        Box::pin(async move {
                            let monitoring = Monitoring::new(builder_signer);
                            let _ = monitoring.run_with_stream(new_canonical_blocks).await;
                        }),
                    );

                    Ok(())
                })
                .launch_with_fn(|builder| {
                    let launcher = EngineNodeLauncher::new(
                        builder.task_executor().clone(),
                        builder.config().datadir(),
                        engine_tree_config,
                    );
                    builder.launch_with(launcher)
                })
                .await?;

            handle.node_exit_future.await
        })
        .unwrap();
}
