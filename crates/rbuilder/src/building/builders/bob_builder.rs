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
use tracing::{debug, error, info, trace, warn};
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

// There is a single bob instance for the entire builder process
// It server as a cache for our event handler loop to store partial blocks
// by uuid, and to store the rpc server responsible for streaming these blocks
// to bob searchers. The bob does not distinguish between partial blocks associated
// between different slots - it should be accessed through a handler which contains
// this association and logic.
#[derive(Clone, Debug)]
pub struct BobBuilder {
    config: BobBuilderConfig,
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
            inner: Arc::new(BobBuilderInner {
                state_diff_server: server,
                block_cache: Mutex::new(block_cache),
            }),
        })
    }

    pub fn server(&self) -> broadcast::Sender<serde_json::Value> {
        return self.inner.state_diff_server.clone();
    }

    // BobBuilder should be accessed through a handler. This
    // handler will be associated with a particular slot, and
    // contains the relevants data fields for it. We spawn
    // a separate process that will wait for the cancellation token
    // associated with the slot, and then perform the necessary tear down.
    // Critically, this includes removing now stale partial blocks by uuid.
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

// Run bob builder is called once at startup of the builder.
// It attached an rpc method to receive bob orders from clients
// then enters and event handler loop that handles incoming orders
// until global cancellation. Blocks are looked up via uuid in cache,
// bob orders applied, and then forwards onto the final sink.
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

            let bundle: Bundle = match raw_bundle.try_into(TxEncoding::WithBlobData) {
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
                    debug!("Received bob order for uuid: {:?} {:?}", order, uuid);
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
                        Ok(Ok(_)) => {
                            sink.new_block(streamed_block);
                        }
                        Ok(Err(e)) => {
                            debug!("Reverted or failed bob order: {:?}", e);
                        }
                        Err(e) => {
                            debug!("Error commiting bob order: {:?}", e);
                        }
                    }
                }
            }
        }
    });

    Ok((handle, module))
}

// BobHandle associate a particular slot to the BobBuilder,
// and store relevant information about the slot, the uuid of partial blocks
// generated for that slot, highest value observed, and final sealer / bidding sink
// The BobBuilder is not accessed directly, it should be only be accessed through the handle.
//
// It implemented the UnfinishedBlockBuilderSink interface so it can act be directly
// used as a sink for other building algorithms.
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
        // If we've processed a cancellation for this slot, bail.
        if self.canceled {
            return;
        }

        // Always pass the block to blocksealingbidder's sink for relay submission
        self.sink.new_block(block.box_clone());

        // We only stream new partial blocks to searchers in a default 2 second window
        // before the slot end. We don't need to store partial blocks not streamed so bail.
        if !self.in_stream_window() {
            return;
        }
        // Only stream new partial blocks whose non-bob value is an increase.
        if !self.check_and_store_block_value(&block) {
            return;
        }
        trace!("Streaming bob partial block");

        let block_uuid = Uuid::new_v4();
        self.uuids.push(block_uuid);

        let building_context = block.building_context();
        let bundle_state = block.get_bundle_state();

        // Create state update object containing block info and state differences
        let block_state_update = BlockStateUpdate {
            block_number: building_context.block_env.number.into(),
            block_timestamp: building_context.block_env.timestamp.into(),
            block_uuid: block_uuid,
            gas_remaining: block.gas_remaining(),
            state_overrides: bundle_state_to_state_overrides(&bundle_state),
        };

        // Insert the block in the builder cache before streaming to searchers
        // in order to avoid a potential race condition where a searcher could respond
        // to the streamed value before it's been inserted and made known to the BobBuilder
        //
        // The actual rpc message is constructed above to avoid creating an uncessary clone
        // due to ownership rules.
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
                debug!("Error getting true block value: {:?}", e);
                return false;
            }
        };
    }

    // Performs teardown for the handle, triggered above when the slot cancellation
    // token is triggered. Removes uuids we've seen from the builder cache.
    //
    // Note that we store that the cancellation has occured - otherwise there could
    // be stale blocks inserted after cancellation due to race conditions in when upstream
    // processed receive / handle cancellation. E.G. our teardown occurs before an upstream builder
    // has handle the cancellation.
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
    gas_remaining: u64,
    state_overrides: StateOverride,
}

// BundleState is the object used to track the accumulated state changes from
// sequential transaction.
//
// We convert this into a StateOverride object, which is a more standard format
// used in other contexts to specifies state overrides, such as in eth_call and
// other simulation methods.
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

            let storage_diff: std::collections::HashMap<B256, B256> = bundle_account
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
