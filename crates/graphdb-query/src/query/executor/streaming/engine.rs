//! Streaming execution engine
//!
//! Simplified single-thread pull-based execution engine.
//! Holds a single root executor and drives it via direct pull (open → next → close).
//! No task scheduling, worker pool, or partitioning machinery.
//!
//! The parallel execution infrastructure (PipelineScheduler, WorkerPool) is kept
//! in the module for reference but is no longer used in the main execution path.

use super::chunk::DataChunk;
use super::executor::StreamingExecutor;
use crate::core::error::QueryError;

/// Streaming execution engine
///
/// Drives a single root executor via direct pull:
/// 1. open() the root executor
/// 2. Loop next() until None
/// 3. close() the root executor
pub struct StreamingExecutionEngine {
    root_executor: Option<Box<StreamingExecutor>>,
}

impl StreamingExecutionEngine {
    /// Create a new streaming execution engine
    pub fn new() -> Self {
        Self {
            root_executor: None,
        }
    }

    /// Register the root executor
    pub fn register_executor(&mut self, _executor_id: usize, executor: StreamingExecutor) {
        self.root_executor = Some(Box::new(executor));
    }

    /// Execute the streaming query via direct single-thread pull
    pub fn execute(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let executor = self
            .root_executor
            .as_mut()
            .ok_or_else(|| QueryError::execution("No executor registered".to_string()))?;

        executor.open()?;

        let mut output_chunks = Vec::new();
        while let Some(chunk) = executor.next()? {
            output_chunks.push(chunk);
        }

        executor.close()?;

        Ok(output_chunks)
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
    fn test_single_scan_executor() {
        let mut engine = StreamingExecutionEngine::new();

        let buffer = create_test_buffer(100);
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
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
        };

        let limit = StreamingExecutor::Limit {
            input: Box::new(scan),
            limit: 10,
            consumed: 0,
            opened: false,
            plan_node_id: 0,
        };

        engine.register_executor(0, limit);

        let result = engine.execute();
        assert!(result.is_ok());
        let chunks = result.unwrap();
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, 10);
    }
}
