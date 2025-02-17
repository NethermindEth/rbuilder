//! order_input handles receiving new orders from the ipc mempool subscription and json rpc server
//!
pub mod order_replacement_manager;
pub mod order_sink;
pub mod orderpool;
pub mod replaceable_order_sink;
pub mod rpc_server;
pub mod txpool_fetcher;

use self::{
    orderpool::{OrderPool, OrderPoolSubscriptionId},
    replaceable_order_sink::ReplaceableOrderSink,
};
use crate::primitives::{serialize::CancelShareBundle, BundleReplacementKey, Order};
use crate::provider::StateProviderFactory;
use crate::telemetry::{set_current_block, set_ordepool_count};
use alloy_consensus::Header;
use jsonrpsee::RpcModule;
use std::{net::Ipv4Addr, path::PathBuf, sync::Arc, time::Duration};
use std::{path::Path, time::Instant};
use tokio::{
    sync::{mpsc, Mutex},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, debug_span, error, info, info_span, trace, warn};

use super::base_config::BaseConfig;

/// Thread safe access to OrderPool to get orderflow
#[derive(Debug)]
pub struct OrderPoolSubscriber {
    orderpool: Arc<Mutex<OrderPool>>,
}

impl OrderPoolSubscriber {
    pub async fn add_sink(
        &self,
        block_number: u64,
        sink: Box<dyn ReplaceableOrderSink>,
    ) -> OrderPoolSubscriptionId {
        let id = self.orderpool.lock().await.add_sink(block_number, sink);
        debug!(
            "Adding sink for block {}, this takes lock, {}",
            block_number, id
        );
        id
    }

    pub async fn remove_sink(
        &self,
        id: &OrderPoolSubscriptionId,
    ) -> Option<Box<dyn ReplaceableOrderSink>> {
        debug!("Removing sink for subs id: {}, this takes lock", id);
        self.orderpool.lock().await.remove_sink(id)
    }
}

/// All the info needed to start all the order related jobs (mempool, rcp, clean)
#[derive(Debug, Clone)]
pub struct OrderInputConfig {
    /// if true - cancellations are disabled.
    ignore_cancellable_orders: bool,
    /// if true -- txs with blobs are ignored
    ignore_blobs: bool,
    /// Path to reth ipc
    ipc_path: Option<PathBuf>,
    /// Input RPC port
    server_port: u16,
    /// Input RPC ip
    server_ip: Ipv4Addr,
    /// Input RPC max connections
    serve_max_connections: u32,
    /// All order sources send new ReplaceableOrderPoolCommands through an mpsc::Sender bounded channel.
    /// Timeout to wait when sending to that channel (after that the ReplaceableOrderPoolCommand is lost).
    results_channel_timeout: Duration,
    /// Size of the bounded channel.
    pub input_channel_buffer_size: usize,
}
pub const DEFAULT_SERVE_MAX_CONNECTIONS: u32 = 4096;
pub const DEFAULT_RESULTS_CHANNEL_TIMEOUT: Duration = Duration::from_millis(50);
pub const DEFAULT_INPUT_CHANNEL_BUFFER_SIZE: usize = 10_000;
impl OrderInputConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ignore_cancellable_orders: bool,
        ignore_blobs: bool,
        ipc_path: Option<PathBuf>,
        server_port: u16,
        server_ip: Ipv4Addr,
        serve_max_connections: u32,
        results_channel_timeout: Duration,
        input_channel_buffer_size: usize,
    ) -> Self {
        Self {
            ignore_cancellable_orders,
            ignore_blobs,
            ipc_path,
            server_port,
            server_ip,
            serve_max_connections,
            results_channel_timeout,
            input_channel_buffer_size,
        }
    }

    pub fn from_config(config: &BaseConfig) -> eyre::Result<Self> {
        let el_node_ipc_path = config
            .el_node_ipc_path
            .as_ref()
            .map(|p| expand_path(p.as_path()))
            .transpose()?;

        Ok(OrderInputConfig {
            ignore_cancellable_orders: config.ignore_cancellable_orders,
            ignore_blobs: config.ignore_blobs,
            ipc_path: el_node_ipc_path,
            server_port: config.jsonrpc_server_port,
            server_ip: config.jsonrpc_server_ip,
            serve_max_connections: 4096,
            results_channel_timeout: Duration::from_millis(50),
            input_channel_buffer_size: 10_000,
        })
    }

    pub fn default_e2e() -> Self {
        Self {
            ipc_path: Some(PathBuf::from("/tmp/anvil.ipc")),
            results_channel_timeout: Duration::new(5, 0),
            ignore_cancellable_orders: false,
            ignore_blobs: false,
            input_channel_buffer_size: 10,
            serve_max_connections: 4096,
            server_ip: Ipv4Addr::new(127, 0, 0, 1),
            server_port: 0,
        }
    }
}

/// Commands we can get from RPC or mempool fetcher.
#[derive(Debug, Clone)]
pub enum ReplaceableOrderPoolCommand {
    /// New or update order
    Order(Order),
    /// Cancellation for sbundle
    CancelShareBundle(CancelShareBundle),
    CancelBundle(BundleReplacementKey),
}

impl ReplaceableOrderPoolCommand {
    pub fn target_block(&self) -> Option<u64> {
        match self {
            ReplaceableOrderPoolCommand::Order(o) => o.target_block(),
            ReplaceableOrderPoolCommand::CancelShareBundle(c) => Some(c.block),
            ReplaceableOrderPoolCommand::CancelBundle(_) => None,
        }
    }
}

/// Starts all the tokio tasks to handle order flow:
/// - Mempool
/// - RPC
/// - Clean up task to remove old stuff.
///
/// @Pending reengineering to modularize rpc, extra_rpc here is a patch to upgrade the created rpc server.
pub async fn start_orderpool_jobs<P>(
    config: OrderInputConfig,
    provider_factory: P,
    extra_rpc: RpcModule<()>,
    global_cancel: CancellationToken,
    order_sender: mpsc::Sender<ReplaceableOrderPoolCommand>,
    order_receiver: mpsc::Receiver<ReplaceableOrderPoolCommand>,
    header_receiver: mpsc::Receiver<Header>,
) -> eyre::Result<(JoinHandle<()>, OrderPoolSubscriber)>
where
    P: StateProviderFactory + 'static,
{
    if config.ignore_cancellable_orders {
        warn!("ignore_cancellable_orders is set to true, some order input is ignored");
    }
    if config.ignore_blobs {
        warn!("ignore_blobs is set to true, some order input is ignored");
    }

    let orderpool = Arc::new(Mutex::new(OrderPool::new()));
    let subscriber = OrderPoolSubscriber {
        orderpool: orderpool.clone(),
    };

    let clean_job = spawn_clean_orderpool_job(
        header_receiver,
        provider_factory,
        orderpool.clone(),
        global_cancel.clone(),
    )?;
    let rpc_server = rpc_server::start_server_accepting_bundles(
        config.clone(),
        order_sender.clone(),
        extra_rpc,
        global_cancel.clone(),
    )
    .await?;

    let mut handles = vec![clean_job, rpc_server];

    if config.ipc_path.is_some() {
        info!("IPC path configured, starting txpool subscription");
        let txpool_fetcher = txpool_fetcher::subscribe_to_txpool_with_blobs(
            config.clone(),
            order_sender.clone(),
            global_cancel.clone(),
        )
        .await?;
        handles.push(txpool_fetcher);
    } else {
        info!("No IPC path configured, skipping txpool subscription");
    }

    let handle = tokio::spawn(async move {
        let id: u64 = rand::random();
        let span = info_span!("Main Orderpool job", id);
        let _guard = span.enter();

        // @Maybe we should add sleep here because each new order will trigger locking
        let mut new_commands = Vec::new();
        let mut order_receiver: mpsc::Receiver<ReplaceableOrderPoolCommand> = order_receiver;

        loop {
            tokio::select! {
                _ = global_cancel.cancelled() => { break; },
                n = order_receiver.recv_many(&mut new_commands, 100) => {
                    if n == 0 {
                        break;
                    }
                },
            };

            // Ignore orders with cancellations if we can't support them
            if config.ignore_cancellable_orders {
                new_commands.retain(|o| {
                    let cancellable_order = match o {
                        ReplaceableOrderPoolCommand::Order(o) => {
                            if o.replacement_key().is_some() {
                                trace!(order=?o.id(), "Ignoring cancellable order (config: ignore_cancellable_orders)")
                            }
                            o.replacement_key().is_some()
                        },
                        ReplaceableOrderPoolCommand::CancelShareBundle(_)|ReplaceableOrderPoolCommand::CancelBundle(_) => true
                    };
                    !cancellable_order
                })
            }

            if config.ignore_blobs {
                new_commands.retain(|o| {
                    let has_blobs = match o {
                        ReplaceableOrderPoolCommand::Order(o) => {
                            if o.has_blobs() {
                                trace!(order=?o.id(), "Ignoring order with blobs (config: ignore_blobs)");
                            }
                            o.has_blobs()
                        },
                        ReplaceableOrderPoolCommand::CancelShareBundle(_)|ReplaceableOrderPoolCommand::CancelBundle(_) => false
                    };
                    !has_blobs
                })
            }

            //debug!("Going to process order pool commands and take the lock");
            //if let Some(mut orderpool) = orderpool.try_lock_for(Duration::from_millis(10)) {
            //    debug!("Got lock");
            //    orderpool.process_commands(new_commands.clone());
            //    debug!("Done orderpoool command processing");
            //    new_commands.clear();
            //}

            debug!("order_pool command processing WAITING FOR LOCK");
            {
                let mut orderpool = orderpool.lock().await;
                debug!("order_pool command processing GOT LOCK");
                orderpool.process_commands(new_commands.clone());
                new_commands.clear();
            }
            debug!("order_pool command processing RELEASED LOCK");
        }

        for handle in handles {
            handle
                .await
                .map_err(|err| {
                    tracing::error!("Error while waiting for OrderPoolJobs to finish: {:?}", err)
                })
                .unwrap_or_default();
        }
        info!("OrderPoolJobs: finished");
    });

    Ok((handle, subscriber))
}

pub fn expand_path(path: &Path) -> eyre::Result<PathBuf> {
    let path_str = path
        .to_str()
        .ok_or_else(|| eyre::eyre!("Invalid UTF-8 in path"))?;

    Ok(PathBuf::from(shellexpand::full(path_str)?.into_owned()))
}

/// Performs maintenance operations on every new header by calling OrderPool::head_updated.
/// Also calls some functions to generate metrics.
fn spawn_clean_orderpool_job<P>(
    header_receiver: mpsc::Receiver<Header>,
    provider_factory: P,
    orderpool: Arc<Mutex<OrderPool>>,
    global_cancellation: CancellationToken,
) -> eyre::Result<JoinHandle<()>>
where
    P: StateProviderFactory + 'static,
{
    let mut header_receiver: mpsc::Receiver<Header> = header_receiver;

    let handle = tokio::spawn(async move {
        info!("Clean orderpool job: started");

        loop {
            tokio::select! {
                header = header_receiver.recv() => {
                    if let Some(header) = header {
                        let block_number = header.number;
                        set_current_block(block_number);

                        let id: u64 = rand::random();
                        let span = info_span!("new head_cleaner", id, block = block_number);
                        let _guard = span.enter();

                        let state = match provider_factory.latest() {
                            Ok(state) => state,
                            Err(err) => {
                                error!("Failed to get latest state: {}", err);
                                // @Metric error count
                                continue;
                            }
                        };

                        let start = Instant::now();
                        //if let Some(mut orderpool) = orderpool.try_lock_for(Duration::from_millis(10)) {




                        {
                            debug!("odrder pool cleaner WAITING FOR LOCK");
                            let mut orderpool = orderpool.lock().await;
                            debug!("odrder pool cleaner GOT LOCK");
                            orderpool.head_updated(block_number, &state);
                            let update_time = start.elapsed();
                            let (tx_count, bundle_count) = orderpool.content_count();

                            set_ordepool_count(tx_count, bundle_count);

                            info!(
                                block_number,
                                tx_count,
                                bundle_count,
                                update_time_ms = update_time.as_millis(),
                                "Cleaned orderpool",
                            );
                        }
                            debug!("odrder pool cleaner RELEASED LOCK");
                        //}
                    } else {
                        info!("Clean orderpool job: channel ended");
                        if !global_cancellation.is_cancelled(){
                            error!("Clean orderpool job: channel ended with no cancellation");
                        }
                        break;
                    }
                },
                _ = global_cancellation.cancelled() => {
                    info!("Clean orderpool job: received cancellation signal");
                    break;
                }
            }
        }

        global_cancellation.cancel();
        info!("Clean orderpool job: finished");
    });
    Ok(handle)
}
