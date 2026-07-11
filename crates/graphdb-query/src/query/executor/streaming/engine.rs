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
/// Drives a single root executor via direct pull:
/// 1. open() the root executor
/// 2. Loop next() until None
/// 3. close() the root executor
///
/// An optional [`ExecutionRuntime`] can be attached to enable cancellation,
/// memory tracking, and profiling.  When present operator calls are routed
/// through [`ExecutorDriver`] which adds uniform cancel checking and
/// profile instrumentation.
pub struct StreamingExecutionEngine {
    root_executor: Option<Box<StreamingExecutor>>,
    runtime: Option<Arc<ExecutionRuntime>>,
}

impl StreamingExecutionEngine {
    /// Create a new streaming execution engine
    pub fn new() -> Self {
        Self {
            root_executor: None,
            runtime: None,
        }
    }

    /// Register the root executor
    pub fn register_executor(&mut self, _executor_id: usize, executor: StreamingExecutor) {
        self.root_executor = Some(Box::new(executor));
    }

    /// Attach an execution runtime (for cancellation, profiling, memory tracking).
    /// Also propagates the runtime recursively into all operators.
    pub fn set_runtime(&mut self, runtime: Arc<ExecutionRuntime>) {
        if let Some(ref mut executor) = self.root_executor {
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
    pub fn execute(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        if let Some(ref rt) = self.runtime {
            rt.profile_start();
        }

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

        if let Some(ref rt) = self.runtime {
            rt.profile_end();
            rt.release_resources();
        }

        // Extract peak memory from executor and record in profile.
        if let Some(ref rt) = self.runtime {
            if let Some(ref root) = self.root_executor {
                let peak = extract_peak_memory(root);
                if peak > 0 {
                    let d = ExecutorDriver::new(rt.clone());
                    d.record_peak_memory(root, peak);
                }
            }
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

/// Extract peak memory from a root executor by inspecting its MemoryTracker,
/// if it is a blocking operator variant.
fn extract_peak_memory(executor: &StreamingExecutor) -> u64 {
    use StreamingExecutor::*;
    match executor {
        Sort { memory_tracker, .. } => memory_tracker.peak() as u64,
        Aggregate { memory_tracker, .. } => memory_tracker.peak() as u64,
        HashJoin { memory_tracker, .. } => memory_tracker.peak() as u64,
        NestedLoopJoin { memory_tracker, .. } => memory_tracker.peak() as u64,
        InnerJoin { memory_tracker, .. } => memory_tracker.peak() as u64,
        LeftJoin { memory_tracker, .. } => memory_tracker.peak() as u64,
        RightJoin { memory_tracker, .. } => memory_tracker.peak() as u64,
        FullOuterJoin { memory_tracker, .. } => memory_tracker.peak() as u64,
        CrossJoin { memory_tracker, .. } => memory_tracker.peak() as u64,
        SemiJoin { memory_tracker, .. } => memory_tracker.peak() as u64,
        GroupBy { memory_tracker, .. } => memory_tracker.peak() as u64,
        Distinct { memory_tracker, .. } => memory_tracker.peak() as u64,
        WindowFunction { memory_tracker, .. } => memory_tracker.peak() as u64,
        Union { memory_tracker, .. } => memory_tracker.peak() as u64,
        Intersect { memory_tracker, .. } => memory_tracker.peak() as u64,
        Except { memory_tracker, .. } => memory_tracker.peak() as u64,
        Minus { memory_tracker, .. } => memory_tracker.peak() as u64,
        TopN { memory_tracker, .. } => memory_tracker.peak() as u64,
        Materialize { memory_tracker, .. } => memory_tracker.peak() as u64,
        _ => 0,
    }
}

impl Default for StreamingExecutionEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::runtime::ExecutionRuntime;
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
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };
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
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };
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
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };
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
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec!["id".to_string(), "name".to_string()],
            plan_node_id: 0,
            runtime: None,
        };
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
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };
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
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        let limit = StreamingExecutor::Limit {
            input: Box::new(scan),
            limit: 10,
            consumed: 0,
            opened: false,
            plan_node_id: 0,
            runtime: None,
        };

        engine.register_executor(0, limit);

        let result = engine.execute();
        assert!(result.is_ok());
        let chunks = result.unwrap();
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, 10);
    }
}
