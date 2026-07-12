//! P8 parallel partition coordinator — EXPERIMENTAL / NOT INTEGRATED.
//!
//! ⚠️  THIS MODULE IS EXPERIMENTAL AND NOT USED IN THE PRODUCTION PATH.
//!
//! Issues preventing production use:
//! - `unsafe impl Send` + `*mut StreamingExecutor` bypasses the borrow
//!   checker without proving thread safety (`P0` in the remediation plan).
//! - The coordinator only handles the legacy `partition_executors` vec,
//!   not the Gather-based `register_partitioned_root` path.
//! - Backpressure is a spin-loop (`yield_now()`) rather than bounded
//!   channels with MemoryBudget integration.
//!
//! `try_execute_partitions_parallel` always returns `Ok(None)` when
//! `max_workers <= 1` (the default), so the parallel branch is never
//! taken in production.  The sequential fallback in
//! `StreamingExecutionEngine::execute_partitions` is used instead.
//!
//! See `docs/plan/executor/streaming_current_remediation_plan.md` and
//! `docs/plan/executor/streaming_p8_integration_plan.md` for the formal
//! parallel integration plan.
//!
//! ## Legacy design (for reference only)
//! Each partition task writes its results into a shared slot protected
//! by a `Mutex`.  The coordinator waits for all tasks to finish via
//! `rayon::scope`, then assembles the output in partition order.
//! On first error, the remaining tasks see a cancelled token and
//! shut down early.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::chunk::DataChunk;
use super::executor::StreamingExecutor;
use super::parallel_safety::is_parallel_safe;
use super::runtime::ExecutionRuntime;
use crate::core::error::QueryError;

/// Profile timing from parallel partition execution.
#[derive(Debug, Default, Clone, Copy)]
pub struct ParallelProfileTiming {
    /// Wall clock time elapsed in the coordinator (microseconds).
    pub wall_time_us: u64,
    /// Sum of individual task wall time — may exceed `wall_time_us`.
    pub work_time_us: u64,
}

impl ParallelProfileTiming {
    pub fn record_wall_time(&mut self, elapsed_us: u64) {
        self.wall_time_us = self.wall_time_us.saturating_add(elapsed_us);
    }
    pub fn record_work_time(&mut self, elapsed_us: u64) {
        self.work_time_us = self.work_time_us.saturating_add(elapsed_us);
    }
}

/// The output produced by a single partition task.
struct PartitionOutput {
    chunks: Vec<DataChunk>,
    error: Option<QueryError>,
    /// Nanoseconds of wall time this partition's task spent executing.
    work_time_ns: u128,
}

impl PartitionOutput {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            error: None,
            work_time_ns: 0,
        }
    }
}

/// Maximum number of buffered chunks per partition before the producing
/// task yields the CPU to allow the coordinator (and other workers) to
/// make progress.  This provides simple backpressure without bounded
/// channels — when the coordinator is also a rayon worker, yielding
/// lets it drain other tasks.
const MAX_BUFFERED_CHUNKS_PER_PARTITION: usize = 8;

/// Attempt to execute partition executors in parallel via rayon.
///
/// Returns `Ok(Some((chunks, profile)))` when parallelism was used.
/// Returns `Ok(None)` when the number of partitions or workers is too
/// low for parallelism — the caller should fall back to the sequential
/// path.
///
/// On the first task error all remaining tasks are cancelled via the
/// runtime's cancel token and the first error is returned.
pub fn try_execute_partitions_parallel(
    partitions: &mut Vec<StreamingExecutor>,
    runtime: Arc<ExecutionRuntime>,
    max_workers: usize,
) -> Result<Option<(Vec<DataChunk>, ParallelProfileTiming)>, QueryError> {
    let n = partitions.len();
    if n <= 1 || max_workers <= 1 {
        return Ok(None);
    }

    for tree in partitions.iter() {
        if !is_parallel_safe(tree) {
            return Ok(None);
        }
    }

    let coordinator_start = Instant::now();

    let outputs: Arc<Vec<Mutex<PartitionOutput>>> =
        Arc::new((0..n).map(|_| Mutex::new(PartitionOutput::new())).collect());
    let cancel_token = runtime.cancel_token();

    let workers = max_workers.min(n);

    // ⚠️  EXPERIMENTAL — NOT FOR PRODUCTION USE.
    //
    // This `unsafe impl Send` bypasses the type system.  Even though
    // `rayon::scope` is synchronous, the approach does not verify that
    // `StreamingExecutor` or its children are actually `Send`.  The P8
    // integration plan requires proper `Send+Sync` proofs and bounded
    // channels before this can be enabled.
    struct RawPtr(*mut StreamingExecutor);
    unsafe impl Send for RawPtr {}

    let base: *mut StreamingExecutor = partitions.as_mut_ptr();
    let worker_groups: Vec<(usize, Vec<RawPtr>)> = {
        let base_chunk = n / workers;
        let remainder = n % workers;
        let mut groups = Vec::with_capacity(workers);
        let mut start = 0;
        for w in 0..workers {
            let count = base_chunk + if w < remainder { 1 } else { 0 };
            if count == 0 {
                break;
            }
            let end = start + count;
            let raw_ptrs: Vec<RawPtr> = (start..end)
                .map(|i| RawPtr(unsafe { base.add(i) }))
                .collect();
            groups.push((start, raw_ptrs));
            start = end;
        }
        groups
    };

    rayon::scope(|s| {
        for (group_start, task_ptrs) in worker_groups {
            let outputs_clone = outputs.clone();
            let cancel = cancel_token.clone();

            s.spawn(move |_| {
                for (offset, RawPtr(raw)) in task_ptrs.into_iter().enumerate() {
                    let pid = group_start + offset;
                    let tree: &mut StreamingExecutor = unsafe { &mut *raw };
                    let task_start = Instant::now();

                    if cancel.load(Ordering::Relaxed) {
                        let _ = tree.close_tree();
                        let elapsed = task_start.elapsed().as_nanos();
                        outputs_clone[pid].lock().unwrap().work_time_ns = elapsed;
                        continue;
                    }

                    match tree.open() {
                        Err(e) => {
                            let mut output = outputs_clone[pid].lock().unwrap();
                            output.error = Some(e);
                            output.work_time_ns = task_start.elapsed().as_nanos();
                            cancel.store(true, Ordering::Relaxed);
                            continue;
                        }
                        Ok(()) => {}
                    }

                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        match tree.advance() {
                            Ok(Some(chunk)) => {
                                let mut output = outputs_clone[pid].lock().unwrap();
                                output.chunks.push(chunk);
                                // Backpressure: yield when a partition has
                                // accumulated enough buffered chunks, so
                                // the coordinator (or other workers) can
                                // make progress.
                                if output.chunks.len() >= MAX_BUFFERED_CHUNKS_PER_PARTITION {
                                    drop(output);
                                    std::thread::yield_now();
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                let mut output = outputs_clone[pid].lock().unwrap();
                                output.error = Some(e);
                                output.work_time_ns = task_start.elapsed().as_nanos();
                                cancel.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                    }

                    let _ = tree.close_tree();
                    let elapsed = task_start.elapsed().as_nanos();
                    outputs_clone[pid].lock().unwrap().work_time_ns = elapsed;
                }
            });
        }
    });

    let wall_time_us = coordinator_start.elapsed().as_micros() as u64;

    // ── Collect results in partition order ───────────────────────
    let mut first_error: Option<QueryError> = None;
    let mut all_chunks = Vec::new();
    let mut total_work_time_us: u64 = 0;

    for (pid, slot) in outputs.iter().enumerate() {
        let mut output = slot.lock().unwrap();
        if let Some(err) = &output.error {
            if first_error.is_none() {
                first_error = Some(QueryError::execution(format!(
                    "Partition {pid} error: {err}",
                )));
            }
        }
        total_work_time_us = total_work_time_us.saturating_add(
            (output.work_time_ns / 1_000) as u64,
        );
        all_chunks.append(&mut output.chunks);
    }

    let profile = ParallelProfileTiming {
        wall_time_us,
        work_time_us: total_work_time_us,
    };

    match first_error {
        Some(e) => Err(e),
        None => Ok(Some((all_chunks, profile))),
    }
}

#[cfg(test)]
mod tests {
    use crate::query::executor::streaming::executor::StreamingExecutor;
    use crate::query::executor::streaming::operator_base::OperatorBase;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::core::Value;

    use super::*;

    fn scan_executor(
        rows: Vec<Vec<Value>>,
        partition_id: usize,
        col_names: Vec<String>,
    ) -> StreamingExecutor {
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                partition_id,
                buffer: rows,
                current_index: 0,
                col_names,
            },
        )
    }

    #[test]
    fn single_partition_falls_back_to_sequential() {
        let mut partitions = vec![scan_executor(
            vec![vec![Value::BigInt(1)]],
            0,
            vec!["id".to_string()],
        )];
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let result = try_execute_partitions_parallel(&mut partitions, runtime, 4);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn max_workers_one_falls_back() {
        let mut partitions = vec![
            scan_executor(vec![vec![Value::BigInt(1)]], 0, vec!["id".to_string()]),
            scan_executor(vec![vec![Value::BigInt(2)]], 1, vec!["id".to_string()]),
        ];
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let result = try_execute_partitions_parallel(&mut partitions, runtime, 1);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn two_partitions_produce_all_rows() {
        let mut partitions = vec![
            scan_executor(
                vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
                0,
                vec!["id".to_string()],
            ),
            scan_executor(
                vec![vec![Value::BigInt(3)], vec![Value::BigInt(4)]],
                1,
                vec!["id".to_string()],
            ),
        ];
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let result = try_execute_partitions_parallel(&mut partitions, runtime, 4);
        let (chunks, profile) = result.unwrap().expect("should use parallel path");
        assert!(profile.wall_time_us > 0);
        assert!(profile.work_time_us > 0);
        let ids: Vec<i64> = chunks
            .iter()
            .flat_map(|c| c.rows.iter())
            .filter_map(|r| match r.first() {
                Some(Value::BigInt(n)) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 4);
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(ids.contains(&3));
        assert!(ids.contains(&4));
    }

    #[test]
    fn three_partitions_preserves_partition_order() {
        let mut partitions = vec![
            scan_executor(vec![vec![Value::BigInt(1)]], 0, vec!["id".to_string()]),
            scan_executor(vec![vec![Value::BigInt(2)]], 1, vec!["id".to_string()]),
            scan_executor(vec![vec![Value::BigInt(3)]], 2, vec!["id".to_string()]),
        ];
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let result = try_execute_partitions_parallel(&mut partitions, runtime, 2);
        let (chunks, _profile) = result.unwrap().expect("should use parallel path");
        let ids: Vec<i64> = chunks
            .iter()
            .flat_map(|c| c.rows.iter())
            .filter_map(|r| match r.first() {
                Some(Value::BigInt(n)) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec![1, 2, 3], "partition order must be preserved");
    }

    #[test]
    fn empty_partitions_produce_empty_result() {
        let mut partitions = vec![
            scan_executor(vec![], 0, vec!["id".to_string()]),
            scan_executor(vec![], 1, vec!["id".to_string()]),
        ];
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let result = try_execute_partitions_parallel(&mut partitions, runtime, 4);
        let (chunks, _profile) = result.unwrap().expect("should use parallel path");
        assert!(chunks.is_empty() || chunks.iter().all(|c| c.rows.is_empty()));
    }

    #[test]
    fn non_parallel_safe_tree_falls_back() {
        let gather = StreamingExecutor::Gather(
            OperatorBase::new(10),
            vec![
                scan_executor(vec![vec![Value::BigInt(1)]], 0, vec![]),
                scan_executor(vec![vec![Value::BigInt(2)]], 1, vec![]),
            ],
            crate::query::executor::streaming::operators::gather_operator::GatherOperator::concatenate(),
        );
        let mut partitions = vec![gather];
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let result = try_execute_partitions_parallel(&mut partitions, runtime, 4);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn profile_timing_is_recorded() {
        let mut partitions = vec![
            scan_executor(vec![vec![Value::BigInt(1)]], 0, vec!["id".to_string()]),
            scan_executor(vec![vec![Value::BigInt(2)]], 1, vec!["id".to_string()]),
        ];
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let result = try_execute_partitions_parallel(&mut partitions, runtime, 4);
        let (_chunks, profile) = result.unwrap().expect("should use parallel path");
        assert!(profile.wall_time_us > 0, "wall time must be > 0");
        assert!(profile.work_time_us > 0, "work time must be > 0");
    }
}
