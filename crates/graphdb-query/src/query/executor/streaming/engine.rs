//! Streaming execution engine
//!
//! Pull-based streaming execution engine.
//!
//! Holds a single root executor and drives it via direct pull
//! (`open → next → close`). The default remains serial. When explicitly
//! configured, formal Gather nodes can use the bounded P8 coordinator for
//! partition-local, parallel-safe child trees.

use std::sync::Arc;

use super::chunk::DataChunk;
use super::executor::{SortDirection, StreamingExecutor};
use super::operator_base::OperatorBase;
use super::operators::blocking_operator::BlockingOperator;
use super::operators::gather_operator::GatherOperator;
use super::runtime::ExecutionRuntime;
use super::stream::ResultStream;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::query::executor::base::{MemoryBudget, MemoryTracker};

const GATHER_NODE_ID: i64 = i64::MIN;
const LOCAL_SORT_NODE_ID: i64 = i64::MIN + 1;
const RIGHT_GATHER_NODE_ID: i64 = i64::MIN + 3;

/// Streaming execution engine
///
/// Drives a single root executor (or multiple partition executors) via
/// direct pull: open() → loop next() → close().
///
/// Partitioned roots retain their normal Gather semantics. Eligible local
/// inputs may run through the bounded P8 coordinator when `max_workers > 1`;
/// all other trees retain the serial pull path.
///
/// An optional [`ExecutionRuntime`] can be attached to enable cancellation,
/// memory tracking, and profiling.  Cancel checking and profile recording
/// are built into [`StreamingExecutor`] dispatch in `executor.rs`.
pub struct StreamingExecutionEngine {
    root_executor: Option<Box<StreamingExecutor>>,
    /// Partition executors for partitioned execution (one per partition).
    partition_executors: Vec<StreamingExecutor>,
    /// Number of local trees currently owned by a Gather root. This remains
    /// meaningful after the legacy `partition_executors` vector is cleared.
    partition_count: usize,
    /// Maximum worker threads for intra-query parallelism (P8).
    /// 1 (default) means fully serial.
    max_workers: usize,
    /// Per-partition output channel capacity for formal P8 Gather nodes.
    max_buffered_chunks: usize,
    runtime: Option<Arc<ExecutionRuntime>>,
}

impl StreamingExecutionEngine {
    /// Create a new streaming execution engine
    pub fn new() -> Self {
        Self {
            root_executor: None,
            partition_executors: Vec::new(),
            partition_count: 0,
            max_workers: 1,
            max_buffered_chunks: 10,
            runtime: None,
        }
    }

    /// Set the maximum number of worker threads for intra-query parallelism.
    ///
    /// Values below one are normalised to the serial fallback.
    pub fn set_max_workers(&mut self, max_workers: usize) {
        self.max_workers = max_workers.max(1);
        self.configure_registered_root_parallelism();
    }

    /// Returns the maximum number of worker threads.
    pub fn max_workers(&self) -> usize {
        self.max_workers
    }

    /// Set the bounded output capacity used by P8 worker channels.
    pub fn set_max_buffered_chunks(&mut self, max_buffered_chunks: usize) {
        self.max_buffered_chunks = max_buffered_chunks.max(1);
        self.configure_registered_root_parallelism();
    }

    /// Register the root executor.
    ///
    /// If a runtime is already attached, it is recursively injected
    /// into the executor tree so that cancellation, profiling, and
    /// resource tracking work regardless of registration order.
    pub fn register_executor(&mut self, _executor_id: usize, executor: StreamingExecutor) {
        if let Some(ref rt) = self.runtime {
            // Must clone the executor before Box to call set_runtime
            let mut boxed = Box::new(executor);
            boxed.set_runtime(Some(rt.clone()));
            self.root_executor = Some(boxed);
        } else {
            self.root_executor = Some(Box::new(executor));
        }
        self.partition_count = 0;
        self.configure_registered_root_parallelism();
    }

    /// Register a root that owns local partition trees below one or more
    /// exchange nodes. Unlike `register_partition_executors`, execution still
    /// starts from a single root, while the recorded count remains available
    /// to profile and explain consumers.
    pub fn register_partitioned_root(
        &mut self,
        partition_count: usize,
        mut executor: StreamingExecutor,
    ) -> Result<(), QueryError> {
        if partition_count == 0 {
            return Err(QueryError::execution(
                "Partitioned root requires at least one partition".to_string(),
            ));
        }
        if let Some(ref runtime) = self.runtime {
            executor.set_runtime(Some(runtime.clone()));
        }
        self.root_executor = Some(Box::new(executor));
        self.partition_executors.clear();
        self.partition_count = partition_count;
        self.configure_registered_root_parallelism();
        Ok(())
    }

    /// Register partition executors (one per partition).
    ///
    /// When set, [`execute`] will run each partition's executor sequentially
    /// and combine the results.  This replaces any single root executor.
    /// If a runtime is already attached, it is recursively injected into
    /// each partition executor.
    pub fn register_partition_executors(&mut self, executors: Vec<StreamingExecutor>) {
        if let Some(ref rt) = self.runtime {
            let mut injected = executors;
            for (partition_id, ex) in injected.iter_mut().enumerate() {
                ex.set_partition_id(partition_id);
                ex.set_runtime(Some(rt.clone()));
            }
            self.partition_executors = injected;
        } else {
            let mut partitioned = executors;
            for (partition_id, executor) in partitioned.iter_mut().enumerate() {
                executor.set_partition_id(partition_id);
            }
            self.partition_executors = partitioned;
        }
        self.root_executor = None;
        self.partition_count = self.partition_executors.len();
    }

    /// Build and register a partitioned executor tree.
    ///
    /// Wraps `local_trees` (one per partition) in a [`GatherOperator`] with
    /// the given `gather_mode`, then optionally attaches a `global_tree` on
    /// top.  The resulting tree is set as the root executor so that normal
    /// [`execute`] and [`into_stream`] paths work unchanged.
    ///
    /// When `global_tree` is `Some`, the tree is:
    ///   global_tree → Gather(local[0], local[1], ...)
    ///
    /// When `global_tree` is `None`, only the gather is used as root.
    pub fn build_partitioned_executor(
        &mut self,
        mut local_trees: Vec<StreamingExecutor>,
        gather_mode: GatherOperator,
        global_tree: Option<StreamingExecutor>,
    ) -> Result<(), QueryError> {
        if local_trees.is_empty() {
            return Err(QueryError::execution(
                "Partitioned execution requires at least one local tree".to_string(),
            ));
        }
        let partition_count = local_trees.len();
        for (partition_id, tree) in local_trees.iter_mut().enumerate() {
            tree.set_partition_id(partition_id);
        }
        let mut gather = StreamingExecutor::Gather(
            OperatorBase::new(GATHER_NODE_ID).with_global(true),
            local_trees,
            gather_mode,
        );

        if let Some(ref rt) = self.runtime {
            gather.set_runtime(Some(rt.clone()));
        }

        if let Some(mut global) = global_tree {
            global.set_global();
            if let Some(ref rt) = self.runtime {
                global.set_runtime(Some(rt.clone()));
            }
            Self::attach_gather_as_child(&mut global, gather)?;
            self.root_executor = Some(Box::new(global));
        } else {
            self.root_executor = Some(Box::new(gather));
        }
        self.partition_executors.clear();
        self.partition_count = partition_count;
        self.configure_registered_root_parallelism();
        Ok(())
    }

    /// Build the only supported partitioned sort shape:
    /// `LocalSort(spec) × N -> Gather::MergeSort(spec, limit)`.
    ///
    /// This entry point prevents callers from feeding arbitrary, unsorted
    /// local streams to the k-way merge implementation. `limit`, when set,
    /// is applied globally after merge order is established.
    pub fn build_partitioned_sort_executor(
        &mut self,
        local_trees: Vec<StreamingExecutor>,
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        limit: Option<usize>,
    ) -> Result<(), QueryError> {
        if sort_expressions.len() != sort_directions.len() {
            return Err(QueryError::execution(
                "Partitioned sort requires one direction per sort expression".to_string(),
            ));
        }
        let budget = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.memory_budget.clone())
            .unwrap_or_else(MemoryBudget::default_budget);
        let local_sorts = local_trees
            .into_iter()
            .map(|input| {
                StreamingExecutor::Blocking(
                    OperatorBase::new(LOCAL_SORT_NODE_ID),
                    Box::new(input),
                    BlockingOperator::Sort {
                        sort_expressions: sort_expressions.clone(),
                        sort_directions: sort_directions.clone(),
                        memory_tracker: MemoryTracker::new(budget.clone()),
                        state: None,
                    },
                )
            })
            .collect();
        self.build_partitioned_executor(
            local_sorts,
            GatherOperator::merge_sort(sort_expressions, sort_directions, limit),
            None,
        )
    }

    /// Build a semantically-correct partitioned join shape:
    /// `Gather(left local trees) -> GlobalJoin <- Gather(right local trees)`.
    ///
    /// This is a gather exchange rather than a hash/range shuffle. It keeps
    /// every join variant correct while the engine remains single-threaded;
    /// a future distributed exchange may replace either gather independently.
    pub fn build_partitioned_join_executor(
        &mut self,
        mut left_local_trees: Vec<StreamingExecutor>,
        mut right_local_trees: Vec<StreamingExecutor>,
        mut global_join: StreamingExecutor,
    ) -> Result<(), QueryError> {
        if left_local_trees.is_empty() || right_local_trees.is_empty() {
            return Err(QueryError::execution(
                "Partitioned join requires at least one partition on both inputs".to_string(),
            ));
        }
        if left_local_trees.len() != right_local_trees.len() {
            return Err(QueryError::execution(format!(
                "Partitioned join input partition counts differ: left={}, right={}",
                left_local_trees.len(),
                right_local_trees.len()
            )));
        }

        let partition_count = left_local_trees.len();
        for (partition_id, tree) in left_local_trees.iter_mut().enumerate() {
            tree.set_partition_id(partition_id);
        }
        for (partition_id, tree) in right_local_trees.iter_mut().enumerate() {
            tree.set_partition_id(partition_id);
        }

        let mut left_gather = StreamingExecutor::Gather(
            OperatorBase::new(GATHER_NODE_ID).with_global(true),
            left_local_trees,
            GatherOperator::concatenate(),
        );
        let mut right_gather = StreamingExecutor::Gather(
            OperatorBase::new(RIGHT_GATHER_NODE_ID).with_global(true),
            right_local_trees,
            GatherOperator::concatenate(),
        );
        if let Some(ref runtime) = self.runtime {
            left_gather.set_runtime(Some(runtime.clone()));
            right_gather.set_runtime(Some(runtime.clone()));
        }

        global_join.set_global();
        if let Some(ref runtime) = self.runtime {
            global_join.set_runtime(Some(runtime.clone()));
        }
        Self::attach_gathers_as_join_children(&mut global_join, left_gather, right_gather)?;

        self.root_executor = Some(Box::new(global_join));
        self.partition_executors.clear();
        self.partition_count = partition_count;
        self.configure_registered_root_parallelism();
        Ok(())
    }

    /// Replace the single input child of a root executor with a gather node.
    fn attach_gather_as_child(
        root: &mut StreamingExecutor,
        gather: StreamingExecutor,
    ) -> Result<(), QueryError> {
        match root {
            StreamingExecutor::Unary(_, input, _)
            | StreamingExecutor::Blocking(_, input, _)
            | StreamingExecutor::Graph(_, input, _)
            | StreamingExecutor::Sink(_, input, _)
            | StreamingExecutor::Ddl(_, input, _)
            | StreamingExecutor::Fulltext(_, input, _)
            | StreamingExecutor::Vector(_, input, _)
            | StreamingExecutor::Txn(_, input, _) => {
                **input = gather;
                Ok(())
            }
            StreamingExecutor::Join(_, left, _, _) => {
                **left = gather;
                Ok(())
            }
            _ => Err(QueryError::execution(
                "The requested global executor cannot accept a Gather input".to_string(),
            )),
        }
    }

    fn attach_gathers_as_join_children(
        root: &mut StreamingExecutor,
        left_gather: StreamingExecutor,
        right_gather: StreamingExecutor,
    ) -> Result<(), QueryError> {
        match root {
            StreamingExecutor::Join(_, left, right, _) => {
                **left = left_gather;
                **right = right_gather;
                Ok(())
            }
            _ => Err(QueryError::execution(
                "Partitioned join requires a Join executor as its global root".to_string(),
            )),
        }
    }

    /// Returns the number of registered partition executors.
    pub fn partition_count(&self) -> usize {
        self.partition_count
    }

    /// Attach an execution runtime (for cancellation, profiling, memory tracking).
    /// Also propagates the runtime recursively into all operators.
    pub fn set_runtime(&mut self, runtime: Arc<ExecutionRuntime>) {
        if let Some(ref mut executor) = self.root_executor {
            executor.set_runtime(Some(runtime.clone()));
        }
        for executor in &mut self.partition_executors {
            executor.set_runtime(Some(runtime.clone()));
        }
        self.runtime = Some(runtime);
        self.configure_registered_root_parallelism();
    }

    fn configure_registered_root_parallelism(&mut self) {
        if let Some(root) = self.root_executor.as_mut() {
            root.configure_parallel_partitions(self.max_workers, self.max_buffered_chunks);
        }
    }

    /// Return a reference to the attached runtime, if any.
    pub fn runtime(&self) -> Option<&Arc<ExecutionRuntime>> {
        self.runtime.as_ref()
    }

    /// Open the root executor (used by [`ResultStream`]).
    pub fn open_root(&mut self) -> Result<(), QueryError> {
        let executor = self
            .root_executor
            .as_mut()
            .ok_or_else(|| QueryError::execution("No executor registered".to_string()))?;
        executor.open()
    }

    /// Pull the next chunk from the root executor (used by [`ResultStream`]).
    pub fn next_chunk_from_root(&mut self) -> Result<Option<DataChunk>, QueryError> {
        let executor = self
            .root_executor
            .as_mut()
            .ok_or_else(|| QueryError::execution("No executor registered".to_string()))?;
        executor.advance()
    }

    /// Stop the root executor (signal upstream to stop producing).
    ///
    /// Used by [`ResultStream`] before close to allow operators to
    /// stop upstream production early.
    pub fn stop_root(&mut self) -> Result<(), QueryError> {
        let mut first_error = None;
        if let Some(ref mut executor) = self.root_executor {
            if let Err(error) = executor.stop_tree() {
                first_error = Some(error);
            }
        }
        for executor in &mut self.partition_executors {
            if let Err(error) = executor.stop_tree() {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Close the root executor (used by [`ResultStream`]).
    pub fn close_root(&mut self) -> Result<(), QueryError> {
        let result = if let Some(ref mut executor) = self.root_executor {
            executor.close_tree()
        } else {
            Ok(())
        };
        if let Some(ref runtime) = self.runtime {
            runtime.release_resources();
        }
        result
    }

    /// Execute the streaming query via direct single-thread pull
    ///
    /// Cancel checking and profile instrumentation are built into
    /// [`StreamingExecutor`] dispatch (`open`/`advance`/`close`).
    ///
    /// If partition executors are registered, each partition is executed
    /// sequentially and the results are concatenated in partition order.
    ///
    /// On any failure the partially-opened executor tree is closed,
    /// profile is ended and runtime resources are released before the
    /// error is returned.
    pub fn execute(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let profile_started = self.runtime.is_some();
        if profile_started {
            self.runtime.as_ref().unwrap().profile_start();
        }

        let result = if !self.partition_executors.is_empty() {
            self.execute_partitions()
        } else {
            self.execute_single()
        };

        // On failure, close any executors that are still open.
        if result.is_err() {
            self.close_open_executors();
        }

        // Always end profile and release resources (success or failure).
        if profile_started {
            if let Some(ref rt) = self.runtime {
                rt.profile_end();
                rt.release_resources();
            }
        }

        // Extract peak memory only on success.
        if result.is_ok() {
            for executor in self
                .partition_executors
                .iter()
                .chain(self.root_executor.iter().map(|e| e.as_ref()))
            {
                let peak = extract_peak_memory(executor);
                if peak > 0 {
                    executor.record_profile_peak_memory(peak);
                }
            }
        }

        result
    }

    /// Close any executors whose `open()` succeeded but whose `close()`
    /// was not called due to a mid-execution error.
    fn close_open_executors(&mut self) {
        if let Some(ref mut executor) = self.root_executor {
            let _ = executor.close_tree();
        }
        for executor in &mut self.partition_executors {
            let _ = executor.close_tree();
        }
    }

    /// Execute the legacy uncomposed partition list sequentially.
    ///
    /// Formal P8 parallelism is intentionally attached to the Gather-based
    /// partitioned root. This compatibility helper has no Gather semantics
    /// and therefore remains serial.
    fn execute_partitions(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let mut all_chunks = Vec::new();
        for executor in &mut self.partition_executors {
            executor.open()?;
            let loop_result = (|| -> Result<(), QueryError> {
                while let Some(chunk) = executor.advance()? {
                    all_chunks.push(chunk);
                }
                Ok(())
            })();
            let close_err = executor.close_tree().err();
            loop_result?;
            if let Some(e) = close_err {
                return Err(e);
            }
        }
        Ok(all_chunks)
    }

    /// Execute a single root executor.
    fn execute_single(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let mut output_chunks = Vec::new();

        let executor = self
            .root_executor
            .as_mut()
            .ok_or_else(|| QueryError::execution("No executor registered".to_string()))?;

        executor.open()?;
        let loop_result = (|| -> Result<(), QueryError> {
            while let Some(chunk) = executor.advance()? {
                output_chunks.push(chunk);
            }
            Ok(())
        })();
        let close_err = executor.close_tree().err();
        loop_result?;
        if let Some(e) = close_err {
            return Err(e);
        }

        Ok(output_chunks)
    }

    /// Convert this engine into a [`ResultStream`] for chunk-at-a-time consumption.
    ///
    /// Requires a runtime to have been set (via [`set_runtime`]).
    /// Returns an error if no runtime has been attached.
    pub fn into_stream(mut self) -> Result<ResultStream, QueryError> {
        let runtime = self
            .runtime
            .take()
            .ok_or_else(|| QueryError::execution("No ExecutionRuntime attached".to_string()))?;
        runtime.profile_start();
        Ok(ResultStream::new(self, runtime))
    }
}

/// Extract peak memory from a root executor.
fn extract_peak_memory(executor: &StreamingExecutor) -> u64 {
    executor.peak_memory_bytes()
}

impl Default for StreamingExecutionEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::operator_base::OperatorBase;
    use super::super::operators::gather_operator::GatherOperator;
    use super::super::operators::source_operator::SourceOperator;
    use super::super::operators::unary_operator::UnaryOperator;
    use super::*;
    use crate::core::Value;

    fn create_test_buffer(count: usize) -> Vec<Vec<Value>> {
        (0..count)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::String(format!("item_{}", i)),
                ]
            })
            .collect()
    }

    fn scan_executor(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> StreamingExecutor {
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                partition_id: 0,
                buffer: rows,
                current_index: 0,
                col_names,
            },
        )
    }

    #[test]
    fn test_engine_creation() {
        let engine = StreamingExecutionEngine::new();
        assert!(engine.root_executor.is_none());
    }

    #[test]
    fn test_engine_with_runtime() {
        let mut engine = StreamingExecutionEngine::new();
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        engine.set_runtime(runtime);

        let buffer = create_test_buffer(50);
        let scan = scan_executor(buffer, vec![]);
        engine.register_executor(0, scan);

        let result = engine.execute();
        assert!(result.is_ok());
        let chunks = result.unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 50);
    }

    #[test]
    fn test_into_stream() {
        let mut engine = StreamingExecutionEngine::new();
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        engine.set_runtime(runtime);

        let buffer = create_test_buffer(10);
        let scan = scan_executor(buffer, vec![]);
        engine.register_executor(0, scan);

        let mut stream = engine.into_stream().unwrap();
        let chunk = stream.next_chunk().unwrap();
        assert!(chunk.is_some());
        assert_eq!(chunk.unwrap().len(), 10);
        let done = stream.next_chunk().unwrap();
        assert!(done.is_none());
    }

    #[test]
    fn test_cancel_during_execution() {
        let mut engine = StreamingExecutionEngine::new();
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        engine.set_runtime(runtime.clone());

        let buffer = create_test_buffer(100);
        let scan = scan_executor(buffer, vec![]);
        engine.register_executor(0, scan);

        // Cancel before execution
        runtime.cancel();
        let result = engine.execute();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }

    #[test]
    fn test_stream_collect() {
        let mut engine = StreamingExecutionEngine::new();
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        engine.set_runtime(runtime);

        let buffer = create_test_buffer(25);
        let scan = scan_executor(buffer, vec!["id".to_string(), "name".to_string()]);
        engine.register_executor(0, scan);

        let stream = engine.into_stream().unwrap();
        let ds = stream.collect().unwrap();
        assert_eq!(ds.row_count(), 25);
        assert_eq!(ds.col_count(), 2);
    }

    #[test]
    fn test_single_scan_executor() {
        let mut engine = StreamingExecutionEngine::new();

        let buffer = create_test_buffer(100);
        let scan = scan_executor(buffer, vec![]);
        engine.register_executor(0, scan);

        let result = engine.execute();
        assert!(result.is_ok());
        let chunks = result.unwrap();
        // 100 rows with chunk size 1024 → single chunk
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 100);
    }

    #[test]
    fn test_filter_limit_pipeline() {
        let mut engine = StreamingExecutionEngine::new();

        let buffer = create_test_buffer(100);
        let scan = Box::new(scan_executor(buffer, vec![]));

        let limit = StreamingExecutor::Unary(
            OperatorBase::new(0),
            scan,
            UnaryOperator::Limit {
                offset: 0,
                limit: 10,
                skipped: 0,
                consumed: 0,
            },
        );

        engine.register_executor(0, limit);

        let result = engine.execute();
        assert!(result.is_ok());
        let chunks = result.unwrap();
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, 10);
    }

    // ── Partition execution tests ──

    fn partitioned_scan_executor(
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

    fn extract_ids(chunks: &[DataChunk]) -> Vec<i64> {
        chunks
            .iter()
            .flat_map(|c| c.rows.iter())
            .filter_map(|row| {
                row.first().and_then(|v| {
                    if let Value::BigInt(id) = v {
                        Some(*id)
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    #[test]
    fn test_partitioned_execution_two_partitions() {
        // 100 rows split into 2 partitions: 0-49 and 50-99
        let all_data = create_test_buffer(100);

        let p0_data: Vec<Vec<Value>> = all_data[0..50].to_vec();
        let p1_data: Vec<Vec<Value>> = all_data[50..100].to_vec();

        let mut engine = StreamingExecutionEngine::new();
        engine.register_partition_executors(vec![
            partitioned_scan_executor(p0_data, 0, vec![]),
            partitioned_scan_executor(p1_data, 1, vec![]),
        ]);

        let result = engine.execute().unwrap();
        let total_rows: usize = result.iter().map(|c| c.len()).sum();
        assert_eq!(total_rows, 100);

        let ids = extract_ids(&result);
        assert_eq!(ids.len(), 100);
        // Verify all 0..99 are present
        for i in 0..100i64 {
            assert!(ids.contains(&i), "Missing id {}", i);
        }
    }

    #[test]
    fn test_partitioned_execution_three_partitions() {
        let all_data = create_test_buffer(99);

        let p0_data: Vec<Vec<Value>> = all_data[0..33].to_vec();
        let p1_data: Vec<Vec<Value>> = all_data[33..66].to_vec();
        let p2_data: Vec<Vec<Value>> = all_data[66..99].to_vec();

        let mut engine = StreamingExecutionEngine::new();
        engine.register_partition_executors(vec![
            partitioned_scan_executor(p0_data, 0, vec![]),
            partitioned_scan_executor(p1_data, 1, vec![]),
            partitioned_scan_executor(p2_data, 2, vec![]),
        ]);

        let result = engine.execute().unwrap();
        let total_rows: usize = result.iter().map(|c| c.len()).sum();
        assert_eq!(total_rows, 99);

        let ids = extract_ids(&result);
        assert_eq!(ids.len(), 99);
        for i in 0..99i64 {
            assert!(ids.contains(&i), "Missing id {}", i);
        }
    }

    #[test]
    fn test_partitioned_execution_with_runtime() {
        let all_data = create_test_buffer(50);
        let p0_data: Vec<Vec<Value>> = all_data[0..25].to_vec();
        let p1_data: Vec<Vec<Value>> = all_data[25..50].to_vec();

        let mut engine = StreamingExecutionEngine::new();
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        engine.set_runtime(runtime);
        engine.register_partition_executors(vec![
            partitioned_scan_executor(p0_data, 0, vec![]),
            partitioned_scan_executor(p1_data, 1, vec![]),
        ]);

        let result = engine.execute().unwrap();
        let total_rows: usize = result.iter().map(|c| c.len()).sum();
        assert_eq!(total_rows, 50);
    }

    #[test]
    fn test_partitioned_execution_equal_to_single() {
        // Verify that partitioned execution produces the same result as single execution
        let all_data = create_test_buffer(100);

        // Single execution
        let mut single_engine = StreamingExecutionEngine::new();
        single_engine.register_executor(0, scan_executor(all_data.clone(), vec![]));
        let single_result = single_engine.execute().unwrap();
        let single_ids = extract_ids(&single_result);

        // Partitioned execution (4 partitions)
        let chunk_size = 25;
        let partition_executors: Vec<StreamingExecutor> = (0..4)
            .map(|p| {
                let start = p * chunk_size;
                let end = ((p + 1) * chunk_size).min(100);
                partitioned_scan_executor(all_data[start..end].to_vec(), p, vec![])
            })
            .collect();

        let mut part_engine = StreamingExecutionEngine::new();
        part_engine.register_partition_executors(partition_executors);
        let part_result = part_engine.execute().unwrap();
        let part_ids = extract_ids(&part_result);

        // Both should contain all 0..99
        assert_eq!(single_ids.len(), part_ids.len());
        let mut sorted_single = single_ids.clone();
        let mut sorted_part = part_ids.clone();
        sorted_single.sort();
        sorted_part.sort();
        assert_eq!(sorted_single, sorted_part);
    }

    #[test]
    fn test_partition_count() {
        let mut engine = StreamingExecutionEngine::new();
        assert_eq!(engine.partition_count(), 0);

        let all_data = create_test_buffer(10);
        engine.register_partition_executors(vec![
            partitioned_scan_executor(all_data[0..5].to_vec(), 0, vec![]),
            partitioned_scan_executor(all_data[5..10].to_vec(), 1, vec![]),
        ]);
        assert_eq!(engine.partition_count(), 2);
    }

    #[test]
    fn gather_root_keeps_partition_count_and_separate_profiles() {
        let mut engine = StreamingExecutionEngine::new();
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        engine.set_runtime(runtime.clone());
        engine
            .build_partitioned_executor(
                vec![
                    partitioned_scan_executor(create_test_buffer(2), 0, vec!["id".to_string()]),
                    partitioned_scan_executor(create_test_buffer(3), 1, vec!["id".to_string()]),
                ],
                GatherOperator::concatenate(),
                None,
            )
            .expect("gather tree should be registered");

        assert_eq!(engine.partition_count(), 2);
        let chunks = engine.execute().expect("gather execution should succeed");
        assert_eq!(chunks.iter().map(DataChunk::len).sum::<usize>(), 5);

        let profile = runtime.profile().lock();
        assert!(profile
            .operators
            .contains_key(&super::super::runtime::OperatorProfileKey::new(0, Some(0))));
        assert!(profile
            .operators
            .contains_key(&super::super::runtime::OperatorProfileKey::new(0, Some(1))));
        assert!(profile
            .operators
            .contains_key(&super::super::runtime::OperatorProfileKey::new(
                i64::MIN,
                None
            )));
    }

    #[test]
    fn p8_parallel_gather_preserves_partition_order_and_bounds_buffers() {
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let mut engine = StreamingExecutionEngine::new();
        engine.set_runtime(runtime.clone());
        engine.set_max_workers(2);
        engine.set_max_buffered_chunks(1);
        engine
            .build_partitioned_executor(
                vec![
                    partitioned_scan_executor(create_test_buffer(1_500), 0, vec!["id".to_string()]),
                    partitioned_scan_executor(
                        (1_500..3_000)
                            .map(|value| {
                                vec![
                                    Value::BigInt(value as i64),
                                    Value::String(format!("item_{value}")),
                                ]
                            })
                            .collect(),
                        1,
                        vec!["id".to_string()],
                    ),
                ],
                GatherOperator::concatenate(),
                None,
            )
            .expect("build parallel gather");

        let chunks = engine.execute().expect("parallel gather execute");
        assert_eq!(
            extract_ids(&chunks),
            (0..3_000).map(|value| value as i64).collect::<Vec<_>>()
        );

        let profile = runtime.profile().lock();
        assert_eq!(profile.parallel_workers, 2);
        assert!(profile.parallel_wall_time_us > 0);
        assert!(profile.parallel_work_time_us > 0);
        assert!(
            profile.parallel_buffered_chunks_peak <= 2,
            "one bounded channel per partition must cap queued chunks"
        );
    }

    #[test]
    fn p8_parallel_gather_cancellation_joins_workers() {
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let mut engine = StreamingExecutionEngine::new();
        engine.set_runtime(runtime.clone());
        engine.set_max_workers(2);
        engine.set_max_buffered_chunks(1);
        engine
            .build_partitioned_executor(
                vec![
                    partitioned_scan_executor(create_test_buffer(5_000), 0, vec!["id".to_string()]),
                    partitioned_scan_executor(create_test_buffer(5_000), 1, vec!["id".to_string()]),
                ],
                GatherOperator::concatenate(),
                None,
            )
            .expect("build parallel gather");

        let mut stream = engine.into_stream().expect("create stream");
        assert!(stream.next_chunk().expect("first chunk").is_some());
        runtime.cancel();
        assert!(stream.next_chunk().is_err());
        assert!(stream.close().is_ok());
        assert!(runtime.profile().lock().parallel_workers > 0);
    }

    #[test]
    fn p8_parallel_merge_gather_preserves_global_sort_order() {
        let runtime = Arc::new(ExecutionRuntime::default_budget());
        let mut engine = StreamingExecutionEngine::new();
        engine.set_runtime(runtime.clone());
        engine.set_max_workers(2);
        engine.set_max_buffered_chunks(1);
        engine
            .build_partitioned_executor(
                vec![
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(1)], vec![Value::BigInt(3)]],
                        0,
                        vec!["id".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(2)], vec![Value::BigInt(4)]],
                        1,
                        vec!["id".to_string()],
                    ),
                ],
                GatherOperator::merge_sort(
                    vec![crate::core::types::expr::Expression::Variable(
                        "id".to_string(),
                    )],
                    vec![SortDirection::Ascending],
                    None,
                ),
                None,
            )
            .expect("build parallel merge gather");

        let chunks = engine.execute().expect("parallel merge gather execute");
        assert_eq!(extract_ids(&chunks), vec![1, 2, 3, 4]);
        assert_eq!(runtime.profile().lock().parallel_workers, 2);
    }

    #[test]
    fn partitioned_sort_builds_local_sorts_before_merging() {
        let mut engine = StreamingExecutionEngine::new();
        engine
            .build_partitioned_sort_executor(
                vec![
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(3)], vec![Value::BigInt(1)]],
                        0,
                        vec!["id".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(4)], vec![Value::BigInt(2)]],
                        1,
                        vec!["id".to_string()],
                    ),
                ],
                vec![crate::core::types::expr::Expression::Variable(
                    "id".to_string(),
                )],
                vec![SortDirection::Ascending],
                Some(3),
            )
            .expect("partitioned sort should build");

        let chunks = engine.execute().expect("partitioned sort should execute");
        assert_eq!(extract_ids(&chunks), vec![1, 2, 3]);
        assert_eq!(chunks[0].col_names(), vec!["id"]);
    }

    #[test]
    fn partitioned_aggregate_runs_once_after_gathering_all_partitions() {
        let mut engine = StreamingExecutionEngine::new();
        let global = StreamingExecutor::Blocking(
            OperatorBase::new(40),
            Box::new(scan_executor(Vec::new(), vec!["amount".to_string()])),
            BlockingOperator::Aggregate {
                group_by_expressions: Vec::new(),
                aggregate_functions: vec![
                    (
                        crate::core::types::operators::AggregateFunction::Count(None),
                        crate::core::types::expr::Expression::Literal(Value::Int(1)),
                    ),
                    (
                        crate::core::types::operators::AggregateFunction::Sum("amount".to_string()),
                        crate::core::types::expr::Expression::Variable("amount".to_string()),
                    ),
                ],
                output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
        );
        engine
            .build_partitioned_executor(
                vec![
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
                        0,
                        vec!["amount".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(3)], vec![Value::BigInt(4)]],
                        1,
                        vec!["amount".to_string()],
                    ),
                ],
                GatherOperator::concatenate(),
                Some(global),
            )
            .expect("partitioned aggregate tree should build");

        let chunks = engine
            .execute()
            .expect("partitioned aggregate should execute");
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].rows,
            vec![vec![Value::BigInt(4), Value::BigInt(10)]]
        );
        assert_eq!(chunks[0].col_names(), vec!["COUNT", "SUM"]);
    }

    #[test]
    fn partitioned_dedup_removes_duplicates_across_partitions() {
        let mut engine = StreamingExecutionEngine::new();
        let global = StreamingExecutor::Blocking(
            OperatorBase::new(41),
            Box::new(scan_executor(Vec::new(), vec!["id".to_string()])),
            BlockingOperator::Distinct {
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
        );
        engine
            .build_partitioned_executor(
                vec![
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
                        0,
                        vec!["id".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(2)], vec![Value::BigInt(3)]],
                        1,
                        vec!["id".to_string()],
                    ),
                ],
                GatherOperator::concatenate(),
                Some(global),
            )
            .expect("partitioned dedup tree should build");

        let chunks = engine.execute().expect("partitioned dedup should execute");
        assert_eq!(extract_ids(&chunks), vec![1, 2, 3]);
        assert_eq!(chunks[0].col_names(), vec!["id"]);
    }

    #[test]
    fn partitioned_limit_applies_offset_and_count_globally() {
        let mut engine = StreamingExecutionEngine::new();
        let global = StreamingExecutor::Unary(
            OperatorBase::new(42),
            Box::new(scan_executor(Vec::new(), vec!["id".to_string()])),
            UnaryOperator::Limit {
                offset: 2,
                limit: 3,
                skipped: 0,
                consumed: 0,
            },
        );
        engine
            .build_partitioned_executor(
                vec![
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(0)], vec![Value::BigInt(1)]],
                        0,
                        vec!["id".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![
                            vec![Value::BigInt(2)],
                            vec![Value::BigInt(3)],
                            vec![Value::BigInt(4)],
                        ],
                        1,
                        vec!["id".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(5)]],
                        2,
                        vec!["id".to_string()],
                    ),
                ],
                GatherOperator::concatenate(),
                Some(global),
            )
            .expect("partitioned limit tree should build");

        let chunks = engine.execute().expect("partitioned limit should execute");
        assert_eq!(extract_ids(&chunks), vec![2, 3, 4]);
    }

    #[test]
    fn partitioned_hash_join_matches_rows_across_partition_boundaries() {
        use super::super::operators::join_operator::JoinOperator;

        let mut engine = StreamingExecutionEngine::new();
        let global_join = StreamingExecutor::Join(
            OperatorBase::new(43),
            Box::new(scan_executor(
                Vec::new(),
                vec!["id".to_string(), "left".to_string()],
            )),
            Box::new(scan_executor(
                Vec::new(),
                vec!["id".to_string(), "right".to_string()],
            )),
            JoinOperator::HashJoin {
                join_condition: None,
                hash_keys: vec![crate::core::types::expr::Expression::Variable(
                    "id".to_string(),
                )],
                probe_keys: vec![crate::core::types::expr::Expression::Variable(
                    "id".to_string(),
                )],
                build_side_hash: std::collections::HashMap::new(),
                all_right_rows: Vec::new(),
                left_consumed: false,
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                right_col_names: Vec::new(),
            },
        );
        engine
            .build_partitioned_join_executor(
                vec![
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(1), Value::String("left-1".to_string())]],
                        0,
                        vec!["id".to_string(), "left".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(2), Value::String("left-2".to_string())]],
                        1,
                        vec!["id".to_string(), "left".to_string()],
                    ),
                ],
                vec![
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(2), Value::String("right-2".to_string())]],
                        0,
                        vec!["id".to_string(), "right".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(1), Value::String("right-1".to_string())]],
                        1,
                        vec!["id".to_string(), "right".to_string()],
                    ),
                ],
                global_join,
            )
            .expect("partitioned hash join tree should build");

        let chunks = engine
            .execute()
            .expect("partitioned hash join should execute");
        assert_eq!(
            chunks
                .iter()
                .flat_map(|chunk| chunk.rows.iter().cloned())
                .collect::<Vec<_>>(),
            vec![
                vec![
                    Value::BigInt(1),
                    Value::String("left-1".to_string()),
                    Value::BigInt(1),
                    Value::String("right-1".to_string()),
                ],
                vec![
                    Value::BigInt(2),
                    Value::String("left-2".to_string()),
                    Value::BigInt(2),
                    Value::String("right-2".to_string()),
                ],
            ]
        );
        for chunk in chunks {
            assert_eq!(chunk.col_names(), vec!["id", "left", "id", "right"]);
        }
    }

    #[test]
    fn partitioned_join_rejects_mismatched_input_partition_counts() {
        use super::super::operators::join_operator::JoinOperator;

        let mut engine = StreamingExecutionEngine::new();
        let global_join = StreamingExecutor::Join(
            OperatorBase::new(44),
            Box::new(scan_executor(Vec::new(), vec!["id".to_string()])),
            Box::new(scan_executor(Vec::new(), vec!["id".to_string()])),
            JoinOperator::InnerJoin {
                join_condition: None,
                build_side_tuples: Vec::new(),
                left_consumed: false,
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                right_col_names: Vec::new(),
            },
        );
        let error = engine
            .build_partitioned_join_executor(
                vec![partitioned_scan_executor(
                    vec![vec![Value::BigInt(1)]],
                    0,
                    vec!["id".to_string()],
                )],
                vec![
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(1)]],
                        0,
                        vec!["id".to_string()],
                    ),
                    partitioned_scan_executor(
                        vec![vec![Value::BigInt(2)]],
                        1,
                        vec!["id".to_string()],
                    ),
                ],
                global_join,
            )
            .expect_err("mismatched partition counts must be rejected");

        assert!(error.to_string().contains("partition counts differ"));
    }

    #[test]
    fn test_register_partition_replaces_root() {
        let mut engine = StreamingExecutionEngine::new();
        let buffer = create_test_buffer(10);
        engine.register_executor(0, scan_executor(buffer, vec![]));
        assert!(engine.root_executor.is_some());

        // Registering partitions should clear root
        engine.register_partition_executors(vec![]);
        assert!(engine.root_executor.is_none());
        assert_eq!(engine.partition_count(), 0);
    }
}
