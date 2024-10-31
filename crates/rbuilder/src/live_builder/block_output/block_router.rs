use alloy_primitives::B256;
use alloy_rpc_types_eth::state::{AccountOverride, StateOverride};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

use crate::building::builders::{
    block_building_helper::BlockBuildingHelper, bob_builder::BobHandle, UnfinishedBlockBuildingSink,
};

#[derive(Debug)]
pub struct UnfinishedBlockRouter {
    bob_handle: Arc<Mutex<BobHandle>>,
    sink: Arc<dyn UnfinishedBlockBuildingSink>,
    slot_timestamp: time::OffsetDateTime,
    state_diff_server: broadcast::Sender<Value>,
    stream_start_dur: Duration,
}

impl UnfinishedBlockRouter {
    pub fn new(
        sink: Arc<dyn UnfinishedBlockBuildingSink>,
        state_diff_server: broadcast::Sender<Value>,
        slot_timestamp: time::OffsetDateTime,
        bob_handle: Arc<Mutex<BobHandle>>,
        stream_start_dur: Duration,
    ) -> Self {
        Self {
            bob_handle: bob_handle,
            sink: sink,
            slot_timestamp: slot_timestamp,
            state_diff_server: state_diff_server,
            stream_start_dur,
        }
    }

    fn should_start_streaming(&self) -> bool {
        let now = time::OffsetDateTime::now_utc();
        let time_into_slot = self.slot_timestamp - now;
        let should_start = time_into_slot >= self.stream_start_dur;

        let ms_into_slot = time_into_slot.whole_milliseconds();
        if ms_into_slot % 100 == 0 {
            tracing::info!(
                slot_timestamp = ?self.slot_timestamp,
                current_time = ?now,
                seconds_into_slot = ms_into_slot / 100,
                should_start,
                "Current time into slot"
            );
        }

        should_start
    }
}

impl UnfinishedBlockBuildingSink for UnfinishedBlockRouter {
    fn new_block(&self, block: Box<dyn BlockBuildingHelper>) {
        if self.should_start_streaming() {
            let building_context = block.building_context();
            let bundle_state = block.get_bundle_state().state();

            // Create a new StateOverride object to store the changes
            let mut pending_state = StateOverride::new();

            // Iterate through each address and account in the bundle state
            for (address, account) in bundle_state.iter() {
                let mut account_override = AccountOverride::default();

                let mut state_diff = HashMap::new();
                for (storage_key, storage_slot) in &account.storage {
                    let key = B256::from(*storage_key);
                    let value = B256::from(storage_slot.present_value);
                    state_diff.insert(key, value);
                }

                if !state_diff.is_empty() {
                    account_override.state_diff = Some(state_diff);
                    pending_state.insert(*address, account_override);
                }
            }

            let uuid = Uuid::new_v4();
            let block_data = json!({
                "blockNumber": building_context.block_env.number,
                "blockTimestamp": building_context.block_env.timestamp,
                "blockUuid": uuid,
                "gasRemaing": block.gas_remaining(),
                "pendingState": pending_state
            });

            if let Err(_e) = self.state_diff_server.send(block_data) {
                warn!("Failed to send block data");
            }

            let now = time::OffsetDateTime::now_utc();
            let ms_into_slot = (now - self.slot_timestamp).whole_milliseconds();

            info!(
                seconds_into_slot = (ms_into_slot as f64) / 1000.0,
                order_count = block.built_block_trace().included_orders.len(),
                "Sent block"
            );

            self.bob_handle
                .lock()
                .unwrap()
                .new_block(block.box_clone(), self.sink.clone(), uuid);
        }

        self.sink.new_block(block);
    }

    fn can_use_suggested_fee_recipient_as_coinbase(&self) -> bool {
        return self.sink.can_use_suggested_fee_recipient_as_coinbase();
    }
}
