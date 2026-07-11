//! Streaming execution engine
//!
//! Simplified single-thread pull-based execution engine.
//! Holds a single root executor and drives it via direct pull (open → next → close).
//! No task scheduling, worker pool, or partitioning machinery.
//!
//! The parallel execution infrastructure (PipelineScheduler, WorkerPool) is kept
//! in the module for reference but is no longer used in the main execution path.

use std::sync::Arc;

use super::chunk::DataChunk;
use super::driver::ExecutorDriver;
use super::executor::StreamingExecutor;
use super::runtime::ExecutionRuntime;
use super::stream::ResultStream;
use crate::core::error::QueryError;

/// Streaming execution engine
///
/// Drives a single root executor (or multiple partition executors) via
/// direct pull: open() → loop next() → close().
///
/// When multiple partition executors are registered, each is executed
/// sequentially in a single thread (no parallelism yet).  This validates
/// partition semantics before introducing multi-threaded execution.
///
/// An optional [`ExecutionRuntime`] can be attached to enable cancellation,
/// memory tracking, and profiling.  When present operator calls are routed
/// through [`ExecutorDriver`] which adds uniform cancel checking and
/// profile instrumentation.
pub struct StreamingExecutionEngine {
    root_executor: Option<Box<StreamingExecutor>>,
    /// Partition executors for partitioned execution (one per partition).
    partition_executors: Vec<StreamingExecutor>,
    runtime: Option<Arc<ExecutionRuntime>>,
}

impl StreamingExecutionEngine {
    /// Create a new streaming execution engine
    pub fn new() -> Self {
        Self {
            root_executor: None,
            partition_executors: Vec::new(),
            runtime: None,
        }
    }

    /// Register the root executor
    pub fn register_executor(&mut self, _executor_id: usize, executor: StreamingExecutor) {
        self.root_executor = Some(Box::new(executor));
    }

    /// Register partition executors (one per partition).
    ///
    /// When set, [`execute`] will run each partition's executor sequentially
    /// and combine the results.  This replaces any single root executor.
    pub fn register_partition_executors(&mut self, executors: Vec<StreamingExecutor>) {
        self.partition_executors = executors;
        self.root_executor = None;
    }

    /// Returns the number of registered partition executors.
    pub fn partition_count(&self) -> usize {
        self.partition_executors.len()
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
        // Clone runtime before borrowing executor to satisfy borrow checker.
        if let Some(ref rt) = self.runtime {
            let d = ExecutorDriver::new(rt.clone());
            d.open(executor)
        } else {
            executor.open()
        }
    }

    /// Pull the next chunk from the root executor (used by [`ResultStream`]).
    pub fn next_chunk_from_root(&mut self) -> Result<Option<DataChunk>, QueryError> {
        let executor = self
            .root_executor
            .as_mut()
            .ok_or_else(|| QueryError::execution("No executor registered".to_string()))?;
        if let Some(ref rt) = self.runtime {
            let d = ExecutorDriver::new(rt.clone());
            d.next(executor)
        } else {
            executor.advance()
        }
    }

    /// Close the root executor (used by [`ResultStream`]).
    pub fn close_root(&mut self) -> Result<(), QueryError> {
        let result = if let Some(ref mut executor) = self.root_executor {
            if let Some(ref rt) = self.runtime {
                let d = ExecutorDriver::new(rt.clone());
                d.close(executor)
            } else {
                executor.close()
            }
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
    /// When a runtime has been attached all operator calls are routed
    /// through [`ExecutorDriver`] which provides cancel checking and
    /// profile instrumentation on every `open`/`next`/`close`.
    ///
    /// If partition executors are registered, each partition is executed
    /// sequentially and the results are concatenated in partition order.
    pub fn execute(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        if let Some(ref rt) = self.runtime {
            rt.profile_start();
        }

        let output_chunks = if !self.partition_executors.is_empty() {
            self.execute_partitions()?
        } else {
            self.execute_single()?
        };

        if let Some(ref rt) = self.runtime {
            rt.profile_end();
            rt.release_resources();
        }

        // Extract peak memory from executor and record in profile.
        if let Some(ref rt) = self.runtime {
            for executor in self.partition_executors.iter().chain(
                self.root_executor.iter().map(|e| e.as_ref()),
            ) {
                let peak = extract_peak_memory(executor);
                if peak > 0 {
                    let d = ExecutorDriver::new(rt.clone());
                    d.record_peak_memory(executor, peak);
                }
            }
        }

        Ok(output_chunks)
    }

    /// Execute all partition executors sequentially, collecting results.
    fn execute_partitions(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let mut all_chunks = Vec::new();
        for executor in &mut self.partition_executors {
            if let Some(ref rt) = self.runtime {
                let d = ExecutorDriver::new(rt.clone());
                d.open(executor)?;
                while let Some(chunk) = d.next(executor)? {
                    all_chunks.push(chunk);
                }
                d.close(executor)?;
            } else {
                executor.open()?;
                while let Some(chunk) = executor.advance()? {
                    all_chunks.push(chunk);
                }
                executor.close()?;
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

        if let Some(ref rt) = self.runtime {
            let d = ExecutorDriver::new(rt.clone());
            d.open(executor)?;
            while let Some(chunk) = d.next(executor)? {
                output_chunks.push(chunk);
            }
            d.close(executor)?;
        } else {
            executor.open()?;
            while let Some(chunk) = executor.advance()? {
                output_chunks.push(chunk);
            }
            executor.close()?;
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
    use super::*;
    use super::super::operators::source_operator::SourceOperator;
    use super::super::operators::unary_operator::UnaryOperator;
    use super::super::operator_base::OperatorBase;
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
                limit: 10,
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
            .filter_map(|row| row.first().and_then(|v| {
                if let Value::BigInt(id) = v { Some(*id) } else { None }
            }))
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
                partitioned_scan_executor(
                    all_data[start..end].to_vec(),
                    p,
                    vec![],
                )
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
