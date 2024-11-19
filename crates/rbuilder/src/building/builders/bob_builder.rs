use crate::{
    building::builders::{BlockBuildingHelper, UnfinishedBlockBuildingSink},
    live_builder::streaming::block_subscription_server::start_block_subscription_server,
    primitives::{
        serialize::{RawBundle, TxEncoding},
        Bundle, Order,
    },
};
use ahash::HashMap;
use alloy_primitives::{B256, U256};
use alloy_rpc_types_eth::state::{AccountOverride, StateOverride};
use futures::FutureExt;
use jsonrpsee::{types::ErrorObject, RpcModule};
use revm::db::BundleState;
use serde::{Deserialize, Serialize};
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

use priority_queue::PriorityQueue;
use crate::primitives::{OrderId, SimulatedOrder};
use crate::building::evm_inspector::UsedStateTrace;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BobBuilderConfig {
    port: u16,
    stream_start_dur: u64,
    channel_timeout: u64,
    channel_buffer_size: usize,
}

impl Default for BobBuilderConfig {
    fn default() -> Self {
        return Self {
            port: 8547,
            stream_start_dur: 2000,
            channel_timeout: 50,
            channel_buffer_size: 100,
        };
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
    stream_start_dur: Duration,
    channel_timeout: Duration,
    channel_buffer_size: usize,
    inner: Arc<BobBuilderInner>,
}

struct BobBuilderInner {
    block_handles: Mutex<HashMap<Uuid, Uuid>>,
    handles: Mutex<HashMap<Uuid, BobHandle>>,
    state_diff_server: broadcast::Sender<serde_json::Value>,
}

impl fmt::Debug for BobBuilderInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BobBuilderInner").finish()
    }
}

impl BobBuilder {
    pub async fn new(config: &BobBuilderConfig, ip: Ipv4Addr) -> eyre::Result<BobBuilder> {
        let server = start_block_subscription_server(ip, config.port)
            .await
            .expect("Failed to start block subscription server");
        let block_handles = HashMap::<Uuid, Uuid>::default();
        let handles = HashMap::<Uuid, BobHandle>::default();
        Ok(Self {
            stream_start_dur: Duration::from_millis(config.stream_start_dur),
            channel_timeout: Duration::from_millis(config.channel_timeout),
            channel_buffer_size: config.channel_buffer_size,
            inner: Arc::new(BobBuilderInner {
                block_handles: Mutex::new(block_handles),
                handles: Mutex::new(handles),
                state_diff_server: server,
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
        let key = Uuid::new_v4();
        let handle = BobHandle {
            inner: Arc::new(BobHandleInner {
                block_cache: Mutex::new(HashMap::<Uuid, Box<dyn BlockBuildingHelper>>::default()),
                builder: self.clone(),
                cancel: cancel.clone(),
                highest_value: Mutex::new(U256::from(0)),
                key: key,
                slot_timestamp: slot_timestamp,
                sink: sink,
                uuids: Mutex::new(Vec::new()),
                bundle_store: Mutex::new(PriorityQueue::new()),
            }),
        };
        self.inner
            .handles
            .lock()
            .unwrap()
            .insert(key, handle.clone());

        // Must remove reference to the handle from the builder when the slot is cancelled.
        let inner = self.inner.clone();
        let _ = cancel.cancelled().map(move |_| {
            let mut handles = inner.handles.lock().unwrap();
            handles.remove(&key)
        });
        return handle;
    }

    pub fn register_block(&self, handle_uuid: Uuid, block_uuid: Uuid) {
        self.inner
            .block_handles
            .lock()
            .unwrap()
            .insert(block_uuid, handle_uuid);
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
    let (order_sender, mut order_receiver) = mpsc::channel(bob_builder.channel_buffer_size);

    let timeout = bob_builder.channel_timeout;
    let mut module = RpcModule::new(());
    module.register_async_method("eth_sendBobBundle", move |params, _| {
        let sender = order_sender.clone();
        async move {
            let start = Instant::now();
            let mut seq = params.sequence();
            let raw_bundle: RawBundle = match seq.next() {
                Ok(bundle) => bundle,
                Err(err) => {
                    debug!(?err, "Failed to parse request: no parameters sent");
                    return Err(ErrorObject::owned(-32601, "Expected 2 parameters", None::<()>));
                }
            };
            let uuid: Uuid = match seq.next() {
                Ok(bundle) => bundle,
                Err(err) => {
                    debug!(?err, "Failed to parse request: 1 of 2 paramters sent");
                    return Err(ErrorObject::owned(-32601, "Expected 2 parameters", None::<()>));
                }
            };

            let bundle: Bundle = match raw_bundle.try_into(TxEncoding::WithBlobData) {
                Ok(bundle) => bundle,
                Err(err) => {
                    debug!(?err, "Failed to parse bundle");
                    return Err(ErrorObject::owned(-32601, "Failed to parse bundle parameter", None::<()>));
                }
            };

            let hash = bundle.hash;
            let order = Order::Bundle(bundle);
            let parse_duration = start.elapsed();
            let target_block = order.target_block().unwrap_or_default();
            trace!(order = ?order.id(), parse_duration_mus = parse_duration.as_micros(), target_block, "Received bundle");
            match sender.send_timeout((order, uuid), timeout).await {
                Ok(()) => { Ok(hash)}
                Err(mpsc::error::SendTimeoutError::Timeout(err)) => {
                    warn!(?err, "Bundle send channel timeout");
                    Err(ErrorObject::owned(-32603, "Internal error: timed out", None::<()>))
                }
                Err(mpsc::error::SendTimeoutError::Closed(err)) => {
                    warn!(?err, "Bundle send channel closed");
                    Err(ErrorObject::owned(-32603, "Internal error: timed out", None::<()>))
                }
            }
        }
    })?;

    let inner = bob_builder.inner.clone();
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    break
                }
                Some((order, block_uuid)) = order_receiver.recv() => {
                    debug!("Received bob order for uuid: {:?} {:?}", order, block_uuid);
                    let handle_uuid = {
                        let block_handles = inner.block_handles.lock().unwrap();
                        block_handles.get(&block_uuid).cloned()
                    };
                    if let Some(handle_uuid) = handle_uuid {
                        let handle = {
                            let handles = inner.handles.lock().unwrap();
                            handles.get(&handle_uuid).cloned()
                        };
                        if let Some(handle) = handle {
                            handle.new_order(order, block_uuid);
                        }
                    };
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
    inner: Arc<BobHandleInner>,
}

impl BobHandle {
    fn new_order(&self, order: Order, uuid: Uuid) {
        let mut block = {
            let block_cache = self.inner.block_cache.lock().unwrap();
            if let Some(block) = block_cache.get(&uuid) {
                block.box_clone()
            } else {
                return;
            }
        };

        match block.commit_order_with_trace(&order) {
            Ok(Ok(execution_result)) => {
                info!(
                    ?uuid,
                    order_id=?order.id(),
                    profit=?execution_result.inplace_sim.coinbase_profit,
                    gas_used=execution_result.gas_used,
                    used_state_trace=?execution_result.used_state_trace,
                    "SUCCESSFULLY COMMITTED ORDER"
                );

                // Insert the order into our bundle cache with appropriate priority
                let profit = execution_result.inplace_sim.coinbase_profit.to::<u128>();
                let sim_order = SimulatedOrder {
                    order: execution_result.order.clone(),
                    sim_value: execution_result.inplace_sim.clone(),
                    prev_order: None,
                    used_state_trace: execution_result.used_state_trace.clone(),
                };
                info!(?sim_order.used_state_trace, "Used state trace");
                // Insert into both cache structures under mutex protection
                {
                    let mut bundle_store = self.inner.bundle_store.lock().unwrap();
                    bundle_store.push(execution_result.order.id(), PrioritizedOrder {
                        order: sim_order,
                        profit,
                    });
                }
                self.inner.sink.new_block(block);
                info!(?uuid, order_id=?order.id(), "STEP 7: ORDER FULLY PROCESSED AND CACHED");
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

impl UnfinishedBlockBuildingSink for BobHandle {
    fn new_block(&self, mut block: Box<dyn BlockBuildingHelper>) {
        // Stream the block to searchers
        self.inner.stream_block(block.box_clone());
        // Try to fill with cached bundle orders
        self.inner.fill_bob_orders(&mut block);
        // Pass filled block to sink
        self.inner.sink.new_block(block);
    }

    fn can_use_suggested_fee_recipient_as_coinbase(&self) -> bool {
        return self.inner.can_use_suggested_fee_recipient_as_coinbase();
    }
}


/// PrioritizedOrder combines a SimulatedOrder with its profit value
/// Used for ordering orders in a priority queue based on profit
#[derive(Debug, Eq, PartialEq)]
struct PrioritizedOrder {
    order: SimulatedOrder,
    profit: u128,
}

/// Implementation of Ord trait to enable comparison/sorting of PrioritizedOrder
/// First compares by profit, then by order ID as a tiebreaker
/// Priorityqueue maintains highest profit first by default
impl Ord for PrioritizedOrder {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.profit
            .cmp(&other.profit)
            .then_with(|| self.order.id().cmp(&other.order.id()))
    }
}

/// PartialOrd implementation is required alongside Ord
/// Since our Ord implementation handles all cases,
/// we can simply wrap the cmp result in Some()
impl PartialOrd for PrioritizedOrder {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct BobHandleInner {
    block_cache: Mutex<HashMap<Uuid, Box<dyn BlockBuildingHelper>>>,
    builder: BobBuilder,
    cancel: CancellationToken,
    highest_value: Mutex<U256>,
    key: Uuid,
    sink: Arc<dyn UnfinishedBlockBuildingSink>,
    slot_timestamp: time::OffsetDateTime,
    uuids: Mutex<Vec<Uuid>>,
    bundle_store: Mutex<PriorityQueue<OrderId, PrioritizedOrder>>,
}

impl fmt::Debug for BobHandleInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BobHandleInner").finish()
    }
}

impl BobHandleInner {
    fn stream_block(&self, block: Box<dyn BlockBuildingHelper>) {
        // If we've processed a cancellation for this slot, bail.
        if self.cancel.is_cancelled() {
            return;
        }

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
        self.uuids.lock().unwrap().push(block_uuid);

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
        self.block_cache.lock().unwrap().insert(block_uuid, block);
        self.builder.register_block(self.key, block_uuid);

        // Get block context and state
        match serde_json::to_value(&block_state_update) {
            Ok(json_data) => {
                if let Err(_e) = self.builder.inner.state_diff_server.send(json_data) {
                    warn!("Failed to send block data");
                } else {
                    // info!(
                    //     "Sent BlockStateUpdate: uuid={}",
                    //     block_state_update.block_uuid
                    // );
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

        return delta < self.builder.stream_start_dur;
    }

    fn check_and_store_block_value(&self, block: &Box<dyn BlockBuildingHelper>) -> bool {
        match block.true_block_value() {
            Ok(value) => {
                let mut highest_value = self.highest_value.lock().unwrap();
                if value > *highest_value {
                    *highest_value = value;
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

    fn validate_storage_reads(
        bundle_state: &BundleState,
        used_state_trace: &UsedStateTrace,
    ) -> bool {
        // Iterate through all read slot values from the simulated order
        for (read_slot_key, value) in &used_state_trace.read_slot_values {
            // Check if the address exists in bundle_state
            if let Some(bundle_account) = bundle_state.state.get(&read_slot_key.address) {
                // If address exists, check if the specific storage slot read still has the same value
                if let Some(storage_slot) = bundle_account.storage.get(&U256::try_from(read_slot_key.key).unwrap()) {
                    let original_value = U256::from_be_bytes(value.0);
                    if storage_slot.present_value != original_value {
                        info!(
                            address = ?read_slot_key.address,
                            slot = ?read_slot_key.key,
                            read_value = ?original_value,
                            current_value = ?storage_slot.present_value,
                            "Storage value changed"
                        );
                        return false;
                    }
                }
                // If storage slot doesn't exist in bundle_state, it means it hasn't changed
                // so we can continue checking other slots
            }
            // If address doesn't exist in bundle_state, it means no changes were made
            // so we can continue checking other slots
        }
        
        // All read slots either match or weren't modified
        true
    }

    fn fill_bob_orders(&self, block: &mut Box<dyn BlockBuildingHelper>) {
        let bundle_store = self.bundle_store.lock().unwrap();
    
        // Try each order in priority order while we have enough gas
        for (_order_id, prioritized_order) in bundle_store.iter() {
            if let Some(ref used_state_trace) = prioritized_order.order.used_state_trace {
                if !Self::validate_storage_reads(block.get_bundle_state(), used_state_trace) {
                    info!(
                        order_id = ?prioritized_order.order.id(),
                        "Skipping order due to storage state changes"
                    );
                    continue;
                }
            }
            
            match block.commit_sim_order(&prioritized_order.order) {
                Ok(Ok(_execution_result)) => {
                    info!(
                        order_id = ?prioritized_order.order.id(),
                        "No storage changes - committed order!"
                    );
                }
                Ok(Err(_err)) => {}
                Err(err) => {
                    info!(
                        ?err,
                        order_id = ?prioritized_order.order.id(),
                        "Critical error committing cached order"
                    );
                }
            }
        }
    }
}

impl Drop for BobHandleInner {
    // Performs teardown for the handle, triggered above when the slot cancellation
    // token is triggered. Removes uuids we've seen from the builder cache.
    //
    // Note that we store that the cancellation has occured - otherwise there could
    // be stale blocks inserted after cancellation due to race conditions in when upstream
    // processed receive / handle cancellation. E.G. our teardown occurs before an upstream builder
    // has handle the cancellation.
    fn drop(&mut self) {
        let mut block_handles = self.builder.inner.block_handles.lock().unwrap();
        self.uuids.lock().unwrap().iter().for_each(|uuid| {
            block_handles.remove(uuid);
        });

        let mut handles = self.builder.inner.handles.lock().unwrap();
        handles.remove(&self.key);
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

            let (balance, nonce) = match bundle_account.is_info_changed() {
                true => (Some(info.balance), Some(info.nonce)),
                false => (None, None),
            };
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
                balance: balance,
                code: code,
                nonce: nonce,
                state_diff: match storage_diff.is_empty() {
                    false => Some(storage_diff),
                    true => None,
                },
                state: None,
            };

            Some((*address, account_override))
        })
        .collect();

    return account_overrides;
}
