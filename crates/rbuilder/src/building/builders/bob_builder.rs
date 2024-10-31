use crate::{
    building::builders::{BlockBuildingHelper, UnfinishedBlockBuildingSink},
    live_builder::{
        config::BobConfig, order_input::OrderInputConfig,
        streaming::block_subscription_server::start_block_subscription_server,
    },
    primitives::{
        serialize::{RawBundle, TxEncoding},
        Bundle, Order,
    },
};
use ahash::HashMap;
use jsonrpsee::RpcModule;
use std::{
    fmt,
    net::Ipv4Addr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct BobBuilderConfig {
    pub port: u16,
    pub ip: Ipv4Addr,
    pub stream_start_dur: Duration,
    pub channel_timeout: Duration,
    pub channel_buffer_size: usize,
}

impl BobBuilderConfig {
    pub fn from_configs(
        bob_config: &BobConfig,
        input_config: &OrderInputConfig,
    ) -> BobBuilderConfig {
        BobBuilderConfig {
            port: bob_config.diff_server_port,
            stream_start_dur: Duration::from_millis(bob_config.stream_start_ms),
            ip: input_config.server_ip,
            channel_timeout: input_config.results_channel_timeout,
            channel_buffer_size: input_config.input_channel_buffer_size,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BobBuilder {
    pub config: BobBuilderConfig,
    pub extra_rpc: RpcModule<()>,
    inner: Arc<BobBuilderInner>,
}

struct BlockCacheEntry {
    block: Box<dyn BlockBuildingHelper>,
    sink: Arc<dyn UnfinishedBlockBuildingSink>,
}

struct BobBuilderInner {
    block_cache: Mutex<HashMap<Uuid, BlockCacheEntry>>,
    state_diff_server: broadcast::Sender<serde_json::Value>,
}

impl fmt::Debug for BobBuilderInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BobBuilderInner").finish()
    }
}

impl BobBuilder {
    pub async fn new(config: BobBuilderConfig) -> eyre::Result<BobBuilder> {
        let server = start_block_subscription_server(config.ip, config.port)
            .await
            .expect("Failed to start block subscription server");
        let block_cache = HashMap::<Uuid, BlockCacheEntry>::default();
        Ok(Self {
            config,
            extra_rpc: RpcModule::new(()),
            inner: Arc::new(BobBuilderInner {
                state_diff_server: server,
                block_cache: Mutex::new(block_cache),
            }),
        })
    }

    pub fn server(&self) -> broadcast::Sender<serde_json::Value> {
        return self.inner.state_diff_server.clone();
    }

    pub fn new_handle(&self, cancel: CancellationToken) -> Arc<Mutex<BobHandle>> {
        let handle = Arc::new(Mutex::new(BobHandle {
            builder: self.clone(),
            uuids: Box::new(Vec::new()),
        }));
        let handle_clone = handle.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {
                    handle_clone.lock().unwrap().remove_uuids();
                }
            }
        });
        return handle;
    }

    pub fn new_block(
        &self,
        block: Box<dyn BlockBuildingHelper>,
        sink: Arc<dyn UnfinishedBlockBuildingSink>,
        uuid: Uuid,
    ) {
        let cache_entry = BlockCacheEntry { block, sink };
        self.inner
            .block_cache
            .lock()
            .unwrap()
            .insert(uuid, cache_entry);
    }
}

pub async fn run_bob_builder(
    bob_builder: &BobBuilder,
    cancel: CancellationToken,
) -> eyre::Result<(JoinHandle<()>, RpcModule<()>)> {
    let (order_sender, mut order_receiver) = mpsc::channel(bob_builder.config.channel_buffer_size);

    let timeout = bob_builder.config.channel_timeout;
    let mut module = RpcModule::new(());
    module.register_async_method("eth_sendBobBundle", move |params, _| {
        let sender = order_sender.clone();
        async move {
            let start = Instant::now();
            let mut seq = params.sequence();
            let raw_bundle: RawBundle = seq.next().unwrap();
            let uuid: Uuid = seq.next().unwrap();

            let bundle: Bundle = match raw_bundle.decode(TxEncoding::WithBlobData) {
                Ok(bundle) => bundle,
                Err(err) => {
                    warn!(?err, "Failed to parse bundle");
                    // @Metric
                    return;
                }
            };

            let order = Order::Bundle(bundle);
            let parse_duration = start.elapsed();
            let target_block = order.target_block().unwrap_or_default();
            trace!(order = ?order.id(), parse_duration_mus = parse_duration.as_micros(), target_block, "Received bundle");
            match sender.send_timeout((order, uuid), timeout).await {
                Ok(()) => {}
                Err(mpsc::error::SendTimeoutError::Timeout(_)) => {}
                Err(mpsc::error::SendTimeoutError::Closed(_)) => {}
            };
        }
    })?;

    let inner = bob_builder.inner.clone();
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    break
                }
                Some((order, uuid)) = order_receiver.recv() => {
                    let cache = inner.block_cache.lock().unwrap();
                    if let Some(entry) =  cache.get(&uuid) {
                        let mut new_block = entry.block.box_clone();
                        match new_block.commit_order(&order) {
                            Ok(_) => {
                                entry.sink.new_block(new_block);
                            }
                            Err(_) => {}
                        }
                    }
                }
            }
        }
    });

    Ok((handle, module))
}

#[derive(Clone, Debug)]
pub struct BobHandle {
    builder: BobBuilder,
    uuids: Box<Vec<Uuid>>,
}

impl BobHandle {
    pub fn new_block(
        &mut self,
        block: Box<dyn BlockBuildingHelper>,
        sink: Arc<dyn UnfinishedBlockBuildingSink>,
        uuid: Uuid,
    ) {
        self.uuids.push(uuid);
        self.builder.new_block(block, sink, uuid);
    }

    pub fn remove_uuids(&self) {
        let mut cache = self.builder.inner.block_cache.lock().unwrap();
        self.uuids.iter().for_each(|uuid| {
            cache.remove(uuid);
        })
    }
}
