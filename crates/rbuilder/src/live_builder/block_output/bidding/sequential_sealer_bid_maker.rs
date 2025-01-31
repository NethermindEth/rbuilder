use crate::live_builder::block_output::relay_submit::BlockBuildingSink;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, debug_span, error};

use super::interfaces::{Bid, BidMaker};

/// BidMaker with a background task sealing only one bid at a time.
/// If several bids arrive while sealing another one we keep only the last one since we assume new is better.
#[derive(Debug)]
pub struct SequentialSealerBidMaker {
    pending_bid: Arc<PendingBid>,
}

impl BidMaker for SequentialSealerBidMaker {
    fn send_bid(&self, bid: Bid) {
        debug!("Sending bid, will update");
        self.pending_bid.update(bid);
    }
}

/// Object used to send new bids to the [SequentialSealerBidMakerProcess].
#[derive(Debug)]
struct PendingBid {
    /// Next bid to send.
    bid: Mutex<Option<Bid>>,
    /// Signaled when we set a new bid.
    bid_notify: Notify,
}

impl PendingBid {
    fn new() -> Self {
        Self {
            bid: Default::default(),
            bid_notify: Notify::new(),
        }
    }
    pub async fn wait_for_change(&self) {
        debug!("Waiting for change");
        self.bid_notify.notified().await;
        debug!("Got  change");
    }
    /// Updates bid, replacing  on current (we assume they are always increasing but we don't check it).
    fn update(&self, bid: Bid) {
        debug!("Updating bid, just before lock");
        *self.bid.lock() = Some(bid);
        debug!("Updating bid, notify");
        self.bid_notify.notify_one();
        debug!("Bid notification sent");
    }

    fn consume_bid(&self) -> Option<Bid> {
        self.bid.lock().take()
    }
}

impl SequentialSealerBidMaker {
    pub fn new(sink: Arc<dyn BlockBuildingSink>, cancel: CancellationToken) -> Self {
        let pending_bid = Arc::new(PendingBid::new());
        let mut sealing_process = SequentialSealerBidMakerProcess {
            sink,
            cancel,
            pending_bid: pending_bid.clone(),
        };

        tokio::task::spawn(async move {
            sealing_process.run().await;
        });
        Self { pending_bid }
    }
}

/// Background task waiting for new bids to seal.
struct SequentialSealerBidMakerProcess {
    /// Destination of the finished blocks.
    sink: Arc<dyn BlockBuildingSink>,
    cancel: CancellationToken,
    pending_bid: Arc<PendingBid>,
}

impl SequentialSealerBidMakerProcess {
    async fn run(&mut self) {
        loop {
            tokio::select! {
                _ = self.pending_bid.wait_for_change() => self.check_for_new_bid().await,
                _ = self.cancel.cancelled() => {
                    debug!("Cancel bid");
                    return;
                }
            }
        }
    }

    /// block.finalize_block + self.sink.new_block inside spawn_blocking.
    async fn check_for_new_bid(&mut self) {
        if let Some(bid) = self.pending_bid.consume_bid() {
            let payout_tx_val = bid.payout_tx_value();
            let block = bid.block();
            let block_number = block.building_context().block();
            let builder_name = block.builder_name().to_string();
            match tokio::task::spawn_blocking(move || {
                let id: u64 = rand::random();
                let span = debug_span!("block_finalizer", id, block_number);
                let _guard = span.enter();

                block.finalize_block(payout_tx_val)
            })
            .await
            {
                Ok(finalize_res) => match finalize_res {
                    Ok(res) => {
                        debug!("Sealed");
                        self.sink.new_block(res.block)
                    }
                    Err(error) => {
                        error!(
                            builder_name,
                            block_number,
                            ?error,
                            "Error on finalize_block on SequentialSealerBidMaker"
                        )
                    }
                },
                Err(error) => error!(
                    block_number,
                    ?error,
                    "Error on join finalize_block on SequentialSealerBidMaker"
                ),
            }
        } else {
            debug!("bid none");
        }
    }
}
