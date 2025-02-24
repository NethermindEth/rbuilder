//! This struct is copied from reth almost as it is https://github.com/paradigmxyz/reth/blob/v1.2.0/crates/payload/builder/src/service.rs
//!
//! Support for building payloads.
//!
//! The payload builder is responsible for building payloads.
//! Once a new payload is created, it is continuously updated.

use alloy_consensus::BlockHeader;
use alloy_rpc_types_engine::PayloadId;
use futures_util::{future::FutureExt, Stream, StreamExt};
use reth_chain_state::CanonStateNotification;
use reth_payload_builder::{
    KeepPayloadJobAlive, PayloadBuilderHandle, PayloadJob, PayloadJobGenerator,
    PayloadServiceCommand,
};
use reth_payload_builder_primitives::{Events, PayloadBuilderError};
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes, PayloadKind, PayloadTypes};
use reth_primitives_traits::NodePrimitives;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, info, trace, warn};

type PayloadFuture<P> = Pin<Box<dyn Future<Output = Result<P, PayloadBuilderError>> + Send + Sync>>;

/// A service that manages payload building tasks.
///
/// This type is an endless future that manages the building of payloads.
///
/// It tracks active payloads and their build jobs that run in a worker pool.
///
/// By design, this type relies entirely on the [`PayloadJobGenerator`] to create new payloads and
/// does know nothing about how to build them, it just drives their jobs to completion.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct PayloadBuilderService<Gen, St, T>
where
    T: PayloadTypes,
    Gen: PayloadJobGenerator,
    Gen::Job: PayloadJob<PayloadAttributes = T::PayloadBuilderAttributes>,
{
    /// The type that knows how to create new payloads.
    generator: Gen,
    /// All active payload jobs.
    payload_jobs: Vec<(Gen::Job, PayloadId)>,
    /// Copy of the sender half, so new [`PayloadBuilderHandle`] can be created on demand.
    service_tx: mpsc::UnboundedSender<PayloadServiceCommand<T>>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand<T>>,
    /// Metrics for the payload builder service
    metrics: PayloadBuilderServiceMetrics,
    /// Chain events notification stream
    chain_events: St,
    /// Payload events handler, used to broadcast and subscribe to payload events.
    payload_events: broadcast::Sender<Events<T>>,
}

const PAYLOAD_EVENTS_BUFFER_SIZE: usize = 20;

// === impl PayloadBuilderService ===

impl<Gen, St, T> PayloadBuilderService<Gen, St, T>
where
    T: PayloadTypes,
    Gen: PayloadJobGenerator,
    Gen::Job: PayloadJob<PayloadAttributes = T::PayloadBuilderAttributes>,
    <Gen::Job as PayloadJob>::BuiltPayload: Into<T::BuiltPayload>,
{
    /// Creates a new payload builder service and returns the [`PayloadBuilderHandle`] to interact
    /// with it.
    ///
    /// This also takes a stream of chain events that will be forwarded to the generator to apply
    /// additional logic when new state is committed. See also
    /// [`PayloadJobGenerator::on_new_state`].
    pub fn new(generator: Gen, chain_events: St) -> (Self, PayloadBuilderHandle<T>) {
        let (service_tx, command_rx) = mpsc::unbounded_channel();
        let (payload_events, _) = broadcast::channel(PAYLOAD_EVENTS_BUFFER_SIZE);

        let service = Self {
            generator,
            payload_jobs: Vec::new(),
            service_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            metrics: Default::default(),
            chain_events,
            payload_events,
        };

        let handle = service.handle();
        (service, handle)
    }

    /// Returns a handle to the service.
    pub fn handle(&self) -> PayloadBuilderHandle<T> {
        PayloadBuilderHandle::new(self.service_tx.clone())
    }

    /// Returns true if the given payload is currently being built.
    fn contains_payload(&self, id: PayloadId) -> bool {
        self.payload_jobs.iter().any(|(_, job_id)| *job_id == id)
    }

    /// Returns the best payload for the given identifier that has been built so far.
    fn best_payload(&self, id: PayloadId) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        let res = self
            .payload_jobs
            .iter()
            .find(|(_, job_id)| *job_id == id)
            .map(|(j, _)| j.best_payload().map(|p| p.into()));
        if let Some(Ok(ref best)) = res {
            self.metrics
                .set_best_revenue(best.block().number(), f64::from(best.fees()));
        }

        res
    }

    /// Returns the best payload for the given identifier that has been built so far and terminates
    /// the job if requested.
    fn resolve(
        &mut self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> Option<PayloadFuture<T::BuiltPayload>> {
        trace!(%id, "resolving payload job");

        let job = self
            .payload_jobs
            .iter()
            .position(|(_, job_id)| *job_id == id)?;
        let (fut, keep_alive) = self.payload_jobs[job].0.resolve_kind(kind);

        if keep_alive == KeepPayloadJobAlive::No {
            let (_, id) = self.payload_jobs.swap_remove(job);
            trace!(%id, "terminated resolved job");
        }

        // Since the fees will not be known until the payload future is resolved / awaited, we wrap
        // the future in a new future that will update the metrics.
        let resolved_metrics = self.metrics.clone();
        let payload_events = self.payload_events.clone();

        let fut = async move {
            let res = fut.await;
            if let Ok(ref payload) = res {
                payload_events
                    .send(Events::BuiltPayload(payload.clone().into()))
                    .ok();

                resolved_metrics
                    .set_resolved_revenue(payload.block().number(), f64::from(payload.fees()));
            }
            res.map(|p| p.into())
        };

        Some(Box::pin(fut))
    }
}

impl<Gen, St, T> PayloadBuilderService<Gen, St, T>
where
    T: PayloadTypes,
    Gen: PayloadJobGenerator,
    Gen::Job: PayloadJob<PayloadAttributes = T::PayloadBuilderAttributes>,
    <Gen::Job as PayloadJob>::BuiltPayload: Into<T::BuiltPayload>,
{
    /// Returns the payload attributes for the given payload.
    fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<<Gen::Job as PayloadJob>::PayloadAttributes, PayloadBuilderError>> {
        let attributes = self
            .payload_jobs
            .iter()
            .find(|(_, job_id)| *job_id == id)
            .map(|(j, _)| j.payload_attributes());

        if attributes.is_none() {
            trace!(%id, "no matching payload job found to get attributes for");
        }

        attributes
    }
}

impl<Gen, St, T, N> Future for PayloadBuilderService<Gen, St, T>
where
    T: PayloadTypes,
    N: NodePrimitives,
    Gen: PayloadJobGenerator + Unpin + 'static,
    <Gen as PayloadJobGenerator>::Job: Unpin + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Gen::Job: PayloadJob<PayloadAttributes = T::PayloadBuilderAttributes>,
    <Gen::Job as PayloadJob>::BuiltPayload: Into<T::BuiltPayload>,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            // notify the generator of new chain events
            while let Poll::Ready(Some(new_head)) = this.chain_events.poll_next_unpin(cx) {
                this.generator.on_new_state(new_head);
            }

            // we poll all jobs first, so we always have the latest payload that we can report if
            // requests
            // we don't care about the order of the jobs, so we can just swap_remove them
            for idx in (0..this.payload_jobs.len()).rev() {
                let (mut job, id) = this.payload_jobs.swap_remove(idx);

                // drain better payloads from the job
                match job.poll_unpin(cx) {
                    Poll::Ready(Ok(_)) => {
                        this.metrics.set_active_jobs(this.payload_jobs.len());
                        trace!(%id, "payload job finished");
                    }
                    Poll::Ready(Err(err)) => {
                        warn!(%err, ?id, "Payload builder job failed; resolving payload");
                        this.metrics.inc_failed_jobs();
                        this.metrics.set_active_jobs(this.payload_jobs.len());
                    }
                    Poll::Pending => {
                        // still pending, put it back
                        this.payload_jobs.push((job, id));
                    }
                }
            }

            // marker for exit condition
            let mut new_job = false;

            // drain all requests
            while let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
                match cmd {
                    PayloadServiceCommand::BuildNewPayload(attr, tx) => {
                        let id = attr.payload_id();
                        let mut res = Ok(id);

                        if this.contains_payload(id) {
                            debug!(%id, parent = %attr.parent(), "Payload job already in progress, ignoring.");
                        } else {
                            // no job for this payload yet, create one
                            let parent = attr.parent();
                            match this.generator.new_payload_job(attr.clone()) {
                                Ok(job) => {
                                    info!(%id, %parent, "New payload job created");
                                    this.metrics.inc_initiated_jobs();
                                    new_job = true;
                                    this.payload_jobs.push((job, id));
                                    this.payload_events
                                        .send(Events::Attributes(attr.clone()))
                                        .ok();
                                }
                                Err(err) => {
                                    this.metrics.inc_failed_jobs();
                                    warn!(%err, %id, "Failed to create payload builder job");
                                    res = Err(err);
                                }
                            }
                        }

                        // return the id of the payload
                        let _ = tx.send(res);
                    }
                    PayloadServiceCommand::BestPayload(id, tx) => {
                        let _ = tx.send(this.best_payload(id));
                    }
                    PayloadServiceCommand::PayloadAttributes(id, tx) => {
                        let attributes = this.payload_attributes(id);
                        let _ = tx.send(attributes);
                    }
                    PayloadServiceCommand::Resolve(id, strategy, tx) => {
                        let _ = tx.send(this.resolve(id, strategy));
                    }
                    PayloadServiceCommand::Subscribe(tx) => {
                        let new_rx = this.payload_events.subscribe();
                        let _ = tx.send(new_rx);
                    }
                }
            }

            if !new_job {
                return Poll::Pending;
            }
        }
    }
}

/// This section is copied from <>
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

/// Payload builder service metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "payloads")]
pub(crate) struct PayloadBuilderServiceMetrics {
    /// Number of active jobs
    pub(crate) active_jobs: Gauge,
    /// Total number of initiated jobs
    pub(crate) initiated_jobs: Counter,
    /// Total number of failed jobs
    pub(crate) failed_jobs: Counter,
    /// Coinbase revenue for best payloads
    pub(crate) best_revenue: Gauge,
    /// Current block returned as the best payload
    pub(crate) best_block: Gauge,
    /// Coinbase revenue for resolved payloads
    pub(crate) resolved_revenue: Gauge,
    /// Current block returned as the resolved payload
    pub(crate) resolved_block: Gauge,
}

impl PayloadBuilderServiceMetrics {
    pub(crate) fn inc_initiated_jobs(&self) {
        self.initiated_jobs.increment(1);
    }

    pub(crate) fn inc_failed_jobs(&self) {
        self.failed_jobs.increment(1);
    }

    pub(crate) fn set_active_jobs(&self, value: usize) {
        self.active_jobs.set(value as f64)
    }

    pub(crate) fn set_best_revenue(&self, block: u64, value: f64) {
        self.best_block.set(block as f64);
        self.best_revenue.set(value)
    }

    pub(crate) fn set_resolved_revenue(&self, block: u64, value: f64) {
        self.resolved_block.set(block as f64);
        self.resolved_revenue.set(value)
    }
}
