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
use alloy_primitives::{B256, U256};
use alloy_rpc_types_eth::state::{AccountOverride, StateOverride};
use jsonrpsee::RpcModule;
use revm::db::BundleState;
use serde::Serialize;
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
use tracing::{error, info, trace, warn};
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

    pub fn new_handle(
        &self,
        sink: Arc<dyn UnfinishedBlockBuildingSink>,
        slot_timestamp: time::OffsetDateTime,
        cancel: CancellationToken,
    ) -> BobHandle {
        let handle = BobHandle {
            inner: Arc::new(Mutex::new(BobHandleInner {
                builder: self.clone(),
                canceled: false,
                highest_value: U256::from(0),
                slot_timestamp: slot_timestamp,
                sink: sink,
                uuids: Vec::new(),
            })),
        };
        let handle_clone = handle.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {
                    handle_clone.inner.lock().unwrap().cancel();
                }
            }
        });
        return handle;
    }

    pub fn insert_block(
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
                    let (streamed_block, sink) = {
                        let cache = inner.block_cache.lock().unwrap();
                        if let Some(entry) = cache.get(&uuid) {
                            (entry.block.box_clone(), entry.sink.clone())
                        } else {
                            continue;
                        }
                    };

                    let mut streamed_block = streamed_block;
                    match streamed_block.commit_order(&order) {
                        Ok(_) => {
                            sink.new_block(streamed_block);
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    });

    Ok((handle, module))
}

#[derive(Clone, Debug)]
pub struct BobHandle {
    inner: Arc<Mutex<BobHandleInner>>,
}

impl UnfinishedBlockBuildingSink for BobHandle {
    fn new_block(&self, block: Box<dyn BlockBuildingHelper>) {
        self.inner.lock().unwrap().pass_and_stream_block(block);
    }

    fn can_use_suggested_fee_recipient_as_coinbase(&self) -> bool {
        return self
            .inner
            .lock()
            .unwrap()
            .can_use_suggested_fee_recipient_as_coinbase();
    }
}

#[derive(Clone, Debug)]
struct BobHandleInner {
    builder: BobBuilder,
    canceled: bool,
    highest_value: U256,
    sink: Arc<dyn UnfinishedBlockBuildingSink>,
    slot_timestamp: time::OffsetDateTime,
    uuids: Vec<Uuid>,
}

impl BobHandleInner {
    fn pass_and_stream_block(&mut self, block: Box<dyn BlockBuildingHelper>) {
        if self.canceled {
            return;
        }

        // Always pass the block to blocksealingbidder's sink for relay submission
        self.sink.new_block(block.box_clone());

        if !self.in_stream_window() {
            return;
        }
        if !self.check_and_store_block_value(&block) {
            return;
        }
        info!("STREAMING BLOCK STATE /n");

        let block_uuid = Uuid::new_v4();
        self.uuids.push(block_uuid);

        let building_context = block.building_context();
        let bundle_state = block.get_bundle_state();

        // Create state update object containing block info and state differences
        let block_state_update = BlockStateUpdate {
            block_number: building_context.block_env.number.into(),
            block_timestamp: building_context.block_env.timestamp.into(),
            block_uuid: block_uuid,
            state_overrides: bundle_state_to_state_overrides(&bundle_state),
        };

        self.builder
            .insert_block(block, self.sink.clone(), block_uuid);

        // Get block context and state
        match serde_json::to_value(&block_state_update) {
            Ok(json_data) => {
                if let Err(_e) = self.builder.inner.state_diff_server.send(json_data) {
                    warn!("Failed to send block data");
                } else {
                    info!(
                        "Sent BlockStateUpdate: uuid={}",
                        block_state_update.block_uuid
                    );
                }
            }
            Err(e) => error!("Failed to serialize block state diff update: {:?}", e),
        }
    }

    fn can_use_suggested_fee_recipient_as_coinbase(&self) -> bool {
        return self.sink.can_use_suggested_fee_recipient_as_coinbase();
    }

    // Checks if we're in the streaming window
    fn in_stream_window(&self) -> bool {
        let now = time::OffsetDateTime::now_utc();
        let delta = self.slot_timestamp - now;

        return delta < self.builder.config.stream_start_dur;
    }

    fn check_and_store_block_value(&mut self, block: &Box<dyn BlockBuildingHelper>) -> bool {
        match block.true_block_value() {
            Ok(value) => {
                if value > self.highest_value {
                    self.highest_value = value;
                    return true;
                }
                return false;
            }
            Err(e) => {
                info!("Error getting true block value: {:?}", e);
                return false;
            }
        };
    }

    pub fn cancel(&mut self) {
        let mut cache = self.builder.inner.block_cache.lock().unwrap();
        self.uuids.iter().for_each(|uuid| {
            cache.remove(uuid);
        });

        self.canceled = true;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlockStateUpdate {
    block_number: U256,
    block_timestamp: U256,
    block_uuid: Uuid,
    state_overrides: StateOverride,
}

fn bundle_state_to_state_overrides(bundle_state: &BundleState) -> StateOverride {
    let account_overrides: StateOverride = bundle_state
        .state
        .iter()
        .filter_map(|(address, bundle_account)| {
            let info = bundle_account.info.as_ref()?;
            if info.is_empty_code_hash() {
                return None;
            }
            let code = bundle_state
                .contracts
                .get(&info.code_hash)
                .map(|code| code.bytes().clone());

            let storage_diff: Vec<(B256, B256)> = bundle_account
                .storage
                .iter()
                .map(|(slot, storage_slot)| {
                    (B256::from(*slot), B256::from(storage_slot.present_value))
                })
                .collect();
            let account_override = AccountOverride {
                balance: Some(info.balance),
                nonce: Some(info.nonce),
                code: code,
                state_diff: if storage_diff.is_empty() {
                    None
                } else {
                    Some(storage_diff)
                },
                state: None,
            };

            Some((*address, account_override))
        })
        .collect();

    return account_overrides;
}
