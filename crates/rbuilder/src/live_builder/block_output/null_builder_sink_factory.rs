//! Null implementation for BuilderSinkFactory. It just throws away the bids.
//! Useful for testing.

use crate::building::builders::Block;
use reth_primitives::format_ether;
use tracing::info;

use super::relay_submit::{BlockBuildingSink, BuilderSinkFactory};

#[derive(Debug)]
pub struct NullBuilderSinkFactory {}

impl BuilderSinkFactory for NullBuilderSinkFactory {
    fn create_builder_sink(
        &self,
        _slot_data: crate::live_builder::payload_events::MevBoostSlotData,
        _competition_bid_value_source: std::sync::Arc<
            dyn super::bid_value_source::interfaces::BidValueSource + Send + Sync,
        >,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Box<dyn super::relay_submit::BlockBuildingSink> {
        Box::new(NullBuilderSink {})
    }
}

#[derive(Debug)]
struct NullBuilderSink {}

impl BlockBuildingSink for NullBuilderSink {
    fn new_block(&self, block: Block) {
        info!(
            true_bid_value = format_ether(block.trace.true_bid_value),
            buidler_name = block.builder_name,
            fill_time_ms = block.trace.fill_time.as_millis(),
            finalize_time_ms = block.trace.finalize_time.as_millis(),
            "NullBuilderSink received new block"
        );
    }
}
