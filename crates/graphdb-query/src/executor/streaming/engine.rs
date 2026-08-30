//! Streaming execution engine
//!
//! Pull-based streaming execution engine.
//!
//! Holds a single root executor and drives it via direct pull
//! (`open → next → close`). The default remains serial. When explicitly
//! configured, formal Gather nodes can use the bounded coordinator for
//! partition-local, parallel-safe child trees.

use std::sync::Arc;

use super::chunk::DataChunk;
use super::executor::{SortDirection, StreamingExecutor};
use super::operators::base::OperatorBase;
use super::operators::blocking::BlockingOperator;
use super::operators::blocking::BlockingOperatorKind;
use super::operators::gather_operator::GatherOperator;
use super::runtime::ExecutionRuntime;
use super::stream::ResultStream;
use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::streaming::plan::types::SyntheticNodeIdAllocator;
use crate::executor::streaming::pool::MorselWorkerPool;
use crate::executor::streaming::spill::{SpillConfig, SpillManager};
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;

mod config;
#[cfg(test)]
mod tests;

/// Streaming execution engine
///
/// Drives a single root executor (or multiple partition executors) via
/// direct pull: open() → loop next() → close().
///
/// Partitioned roots retain their normal Gather semantics. Eligible local
/// inputs may run through the bounded coordinator when `max_workers > 1`;
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
    /// Maximum worker threads for intra-query parallelism.
    /// 1 (default) means fully serial.
    max_workers: usize,
    /// Per-partition output channel capacity for formal Gather nodes.
    max_buffered_chunks: usize,
    runtime: Option<Arc<ExecutionRuntime>>,
    /// Allocator for synthetic node IDs (Gather, Start sources, etc.).
    /// Replaces hardcoded sentinel values to avoid collision with real IDs.
    synthetic_id_alloc: SyntheticNodeIdAllocator,
    /// Spill configuration for blocking operators that support disk spill.
    spill_config: SpillConfig,
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
            synthetic_id_alloc: SyntheticNodeIdAllocator::new(),
            spill_config: SpillConfig::default(),
        }
    }

    /// Set the maximum number of worker threads for intra-query parallelism.
    ///
    /// Values below one are normalised to the serial fallback.
    /// When a runtime is already attached and no shared scheduler is
    /// configured, this creates a per-query `MorselWorkerPool`.
    pub fn set_max_workers(&mut self, max_workers: usize) {
        self.max_workers = max_workers.max(1);
        if let Some(rt) = &self.runtime {
            if rt.get_shared_scheduler().is_some() {
                return;
            }
            if self.max_workers > 1 {
                let pool = MorselWorkerPool::new(self.max_workers);
                rt.set_worker_pool(Some(pool));
            } else {
                rt.set_worker_pool(None);
            }
        }
    }

    /// Returns the maximum number of worker threads.
    pub fn max_workers(&self) -> usize {
        self.max_workers
    }

    /// Set the spill configuration for blocking operators.
    pub fn set_spill_config(&mut self, config: SpillConfig) {
        self.spill_config = config;
        if let Some(rt) = &self.runtime {
            let qid = rt.query_id().query_id;
            if let Ok(manager) = SpillManager::new(self.spill_config.clone(), qid) {
                rt.set_spill_manager(Some(Arc::new(manager)));
            }
        }
    }

    /// Set the bounded output capacity used by worker channels.
    pub fn set_max_buffered_chunks(&mut self, max_buffered_chunks: usize) {
        self.max_buffered_chunks = max_buffered_chunks.max(1);
        if let Some(rt) = &self.runtime {
            rt.set_max_buffered_chunks(self.max_buffered_chunks);
        }
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
        self.ensure_partition_arenas();
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
        self.ensure_partition_arenas();
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
        let output_layout = Arc::clone(&local_trees[0].base().output_layout);
        for (partition_id, tree) in local_trees.iter_mut().enumerate() {
            tree.set_partition_id(partition_id);
        }
        let gather_id = self.synthetic_id_alloc.allocate();
        let mut gather = StreamingExecutor::Gather(
            OperatorBase::new(gather_id)
                .with_global(true)
                .with_output_layout(output_layout),
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
        self.ensure_partition_arenas();
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
        let local_sort_id = self.synthetic_id_alloc.allocate();
        let local_sorts = local_trees
            .into_iter()
            .map(|input| {
                let output_layout = Arc::clone(&input.base().output_layout);
                StreamingExecutor::Blocking(
                    OperatorBase::new(local_sort_id).with_output_layout(output_layout.clone()),
                    Box::new(input),
                    BlockingOperator::new(
                        BlockingOperatorKind::Sort {
                            sort_expressions: sort_expressions.clone(),
                            sort_directions: sort_directions.clone(),
                            memory_tracker: MemoryTracker::new(budget.clone()),
                            state: None,
                        },
                        output_layout,
                    ),
                )
            })
            .collect::<Vec<_>>();
        let gather_layout = local_sorts
            .first()
            .map(|tree| Arc::clone(&tree.base().output_layout))
            .ok_or_else(|| {
                QueryError::execution(
                    "Partitioned sort requires at least one local tree".to_string(),
                )
            })?;
        self.build_partitioned_executor(
            local_sorts,
            GatherOperator::merge_sort(sort_expressions, sort_directions, limit, gather_layout),
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
        let left_layout = Arc::clone(&left_local_trees[0].base().output_layout);
        let right_layout = Arc::clone(&right_local_trees[0].base().output_layout);
        for (partition_id, tree) in left_local_trees.iter_mut().enumerate() {
            tree.set_partition_id(partition_id);
        }
        for (partition_id, tree) in right_local_trees.iter_mut().enumerate() {
            tree.set_partition_id(partition_id);
        }

        let left_gather_id = self.synthetic_id_alloc.allocate();
        let right_gather_id = self.synthetic_id_alloc.allocate();
        let mut left_gather = StreamingExecutor::Gather(
            OperatorBase::new(left_gather_id)
                .with_global(true)
                .with_output_layout(left_layout.clone()),
            left_local_trees,
            GatherOperator::concatenate(left_layout),
        );
        let mut right_gather = StreamingExecutor::Gather(
            OperatorBase::new(right_gather_id)
                .with_global(true)
                .with_output_layout(right_layout.clone()),
            right_local_trees,
            GatherOperator::concatenate(right_layout),
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
        self.ensure_partition_arenas();
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
            | StreamingExecutor::RecursiveFragment(_, input, _)
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

    /// Ensure runtime state_arenas match the engine's partition count.
    /// Best-effort: only succeeds when the Arc is unique at call time.
    fn ensure_partition_arenas(&mut self) {
        if let Some(arc) = self.runtime.as_mut() {
            if let Some(rt) = Arc::get_mut(arc) {
                rt.set_partition_count(self.partition_count + 1);
            }
        }
    }

    /// Attach an execution runtime (for cancellation, profiling, memory tracking).
    /// Also propagates the runtime recursively into all operators.
    /// If `max_workers > 1` and no shared scheduler is configured, creates
    /// a per-query `MorselWorkerPool`.
    pub fn set_runtime(&mut self, runtime: Arc<ExecutionRuntime>) {
        if let Some(ref mut executor) = self.root_executor {
            executor.set_runtime(Some(runtime.clone()));
        }
        for executor in &mut self.partition_executors {
            executor.set_runtime(Some(runtime.clone()));
        }
        if self.max_workers > 1 && runtime.get_shared_scheduler().is_none() {
            let pool = MorselWorkerPool::new(self.max_workers);
            runtime.set_worker_pool(Some(pool));
        } else if self.max_workers == 1 && runtime.get_shared_scheduler().is_none() {
            runtime.set_worker_pool(None);
        }
        runtime.set_max_buffered_chunks(self.max_buffered_chunks);
        if runtime.get_spill_manager().is_none() {
            let qid = runtime.query_id().query_id;
            if let Ok(manager) = SpillManager::new(self.spill_config.clone(), qid) {
                runtime.set_spill_manager(Some(Arc::new(manager)));
            }
        }
        self.runtime = Some(runtime);
        // Safety net: ensure arenas match partition count if the Arc
        // is still unique at this point (e.g. when called before any clones).
        self.ensure_partition_arenas();
    }

    fn configure_registered_root_parallelism(&mut self) {
        // No longer needed — parallel config is stored on the runtime.
        // Kept as a hook for future per-operator config.
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
        let mut chunk = executor.advance()?;
        // Materialize any propagated selection before the chunk leaves the
        // executor tree (the API layer consumes full rows).
        if let Some(chunk) = chunk.as_mut() {
            chunk.materialize_selection_by("Root");
        }
        Ok(chunk)
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
            runtime.reset_arena();
        }
        result
    }

    /// Execute the query and return a streaming result handle.
    ///
    /// This is the default execution path. Results are delivered chunk-at-a-time
    /// via [`ResultStream`], enabling:
    /// - Low first-chunk latency (no need to wait for full materialization)
    /// - Constant memory usage regardless of result size
    /// - Cooperative cancellation mid-stream
    ///
    /// For cases that require full materialization (e.g. EXPLAIN/PROFILE
    /// diagnostics, test assertions), use [`execute_collected`](Self::execute_collected).
    ///
    /// # Partitioned Execution
    /// When partition executors are registered, partitions are executed
    /// sequentially and the collected chunks are wrapped in a stream.
    /// Formal parallelism runs through the Gather-based root.
    pub fn execute(mut self) -> Result<ResultStream, QueryError> {
        if !self.partition_executors.is_empty() {
            let chunks = self.execute_collected()?;
            let runtime = self
                .runtime
                .take()
                .ok_or_else(|| QueryError::execution("No ExecutionRuntime attached".to_string()))?;
            runtime.profile_start();
            return Ok(ResultStream::from_collected(chunks, self, runtime));
        }
        self.into_stream()
    }

    /// Execute and collect all chunks into memory (explicit materialization).
    ///
    /// Use this when you need the full result set upfront. For streaming
    /// consumption, prefer [`execute`](Self::execute).
    pub fn execute_collected(&mut self) -> Result<Vec<DataChunk>, QueryError> {
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
                rt.reset_arena();
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
    /// Formal parallelism is intentionally attached to the Gather-based
    /// partitioned root. This compatibility helper has no Gather semantics
    /// and therefore remains serial.
    ///
    /// M0.7: if the main execution loop succeeds but close fails, the close
    /// error is returned instead of being silently logged.
    fn execute_partitions(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let mut all_chunks = Vec::new();
        for executor in &mut self.partition_executors {
            executor.open()?;
            let loop_result = (|| -> Result<(), QueryError> {
                while let Some(mut chunk) = executor.advance()? {
                    chunk.materialize_selection_by("Engine");
                    all_chunks.push(chunk);
                }
                Ok(())
            })();
            let close_err = executor.close_tree().err();
            // M0.7: propagate close error when main loop succeeded.
            loop_result?;
            if let Some(e) = close_err {
                return Err(e);
            }
        }
        Ok(all_chunks)
    }

    /// Execute a single root executor.
    ///
    /// M0.7: if the main execution loop succeeds but close fails, the close
    /// error is returned instead of being silently logged.
    fn execute_single(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let mut output_chunks = Vec::new();

        let executor = self
            .root_executor
            .as_mut()
            .ok_or_else(|| QueryError::execution("No executor registered".to_string()))?;

        executor.open()?;
        let loop_result = (|| -> Result<(), QueryError> {
            while let Some(mut chunk) = executor.advance()? {
                chunk.materialize_selection_by("Engine");
                output_chunks.push(chunk);
            }
            Ok(())
        })();
        let close_err = executor.close_tree().err();
        // M0.7: propagate close error when main loop succeeded.
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
