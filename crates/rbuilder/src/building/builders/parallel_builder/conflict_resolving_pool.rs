use alloy_primitives::utils::format_ether;
use crossbeam_queue::SegQueue;
use eyre::Result;
use std::{
    sync::{mpsc as std_mpsc, Arc},
    thread,
    time::Instant,
};
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

use super::{
    conflict_resolvers::ResolverContext, conflict_task_generator::get_tasks_for_group,
    simulation_cache::SharedSimulationCache, ConflictGroup, ConflictResolutionResultPerGroup,
    ConflictTask, GroupId, ResolutionResult, TaskPriority,
};
use crate::building::BlockBuildingContext;
use crate::provider::StateProviderFactory;

pub type TaskQueue = Arc<SegQueue<ConflictTask>>;

pub struct ConflictResolvingPool<P> {
    task_queue: TaskQueue,
    group_result_sender: std_mpsc::Sender<ConflictResolutionResultPerGroup>,
    cancellation_token: CancellationToken,
    ctx: BlockBuildingContext,
    provider: P,
    simulation_cache: Arc<SharedSimulationCache>,
    num_threads: usize,
}

impl<P> ConflictResolvingPool<P>
where
    P: StateProviderFactory + Clone + 'static,
{
    pub fn new(
        num_threads: usize,
        task_queue: TaskQueue,
        group_result_sender: std_mpsc::Sender<ConflictResolutionResultPerGroup>,
        cancellation_token: CancellationToken,
        ctx: BlockBuildingContext,
        provider: P,
        simulation_cache: Arc<SharedSimulationCache>,
    ) -> Self {
        Self {
            task_queue,
            group_result_sender,
            cancellation_token,
            ctx,
            provider,
            simulation_cache,
            num_threads,
        }
    }

    pub fn start(&self) {
        for _ in 0..self.num_threads {
            let task_queue = self.task_queue.clone();
            let cancellation_token = self.cancellation_token.clone();
            let provider = self.provider.clone();
            let group_result_sender = self.group_result_sender.clone();
            let simulation_cache = self.simulation_cache.clone();
            let ctx = self.ctx.clone();

            thread::spawn(move || {
                while !cancellation_token.is_cancelled() {
                    if let Some(task) = task_queue.pop() {
                        if cancellation_token.is_cancelled() {
                            return;
                        }
                        let task_start = Instant::now();
                        if let Ok((task_id, result)) = Self::process_task(
                            task,
                            &ctx,
                            &provider,
                            cancellation_token.clone(),
                            Arc::clone(&simulation_cache),
                        ) {
                            match group_result_sender.send((task_id, result)) {
                                Ok(_) => {
                                    trace!(
                                        task_id = %task_id,
                                        time_taken_ms = %task_start.elapsed().as_millis(),
                                        "Conflict resolving: successfully sent group result"
                                    );
                                }
                                Err(err) => {
                                    warn!(
                                        task_id = %task_id,
                                        error = ?err,
                                        time_taken_ms = %task_start.elapsed().as_millis(),
                                        "Conflict resolving: failed to send group result"
                                    );
                                    return;
                                }
                            }
                        }
                    }
                }
            });
        }
    }

    fn process_task(
        task: ConflictTask,
        ctx: &BlockBuildingContext,
        provider: &P,
        cancellation_token: CancellationToken,
        simulation_cache: Arc<SharedSimulationCache>,
    ) -> Result<(GroupId, (ResolutionResult, ConflictGroup))> {
        let mut merging_context = ResolverContext::new(
            provider.clone(),
            ctx.clone(),
            cancellation_token.clone(),
            None,
            simulation_cache,
        );
        let task_id = task.group_idx;
        let task_group = task.group.clone();
        let task_algo = task.algorithm;

        match merging_context.run_conflict_task(task) {
            Ok(sequence_of_orders) => {
                trace!(
                    task_type = ?task_algo,
                    group_id = task_id,
                    profit = format_ether(sequence_of_orders.total_profit),
                    order_count = sequence_of_orders.sequence_of_orders.len(),
                    "Successfully ran conflict task"
                );
                Ok((task_id, (sequence_of_orders, task_group)))
            }
            Err(err) => {
                // Fast patch/heuristic to fix excessive tracing.
                // TODO: Use good errors.
                if !cancellation_token.is_cancelled() {
                    warn!(
                        group_id = task_id,
                        err = ?err,
                        "Error running conflict task for group_idx",
                    );
                }
                Err(err)
            }
        }
    }

    pub fn process_groups_backtest(
        &mut self,
        new_groups: Vec<ConflictGroup>,
        ctx: &BlockBuildingContext,
        provider: &P,
        simulation_cache: Arc<SharedSimulationCache>,
    ) -> Vec<(GroupId, (ResolutionResult, ConflictGroup))> {
        let mut results = Vec::new();
        for new_group in new_groups {
            let tasks = get_tasks_for_group(&new_group, TaskPriority::High);
            for task in tasks {
                let simulation_cache = Arc::clone(&simulation_cache);
                let result = Self::process_task(
                    task,
                    ctx,
                    provider,
                    CancellationToken::new(),
                    simulation_cache,
                );
                if let Ok(result) = result {
                    results.push(result);
                }
            }
        }
        results
    }
}
