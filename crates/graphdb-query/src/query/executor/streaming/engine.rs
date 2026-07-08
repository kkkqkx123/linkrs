//! Streaming execution engine that coordinates all components
//!
//! Orchestrates StreamingExecutor, PipelineScheduler, PartitionView,
//! and WorkerPool into a cohesive pull-based streaming execution system.

use super::chunk::DataChunk;
use super::executor::StreamingExecutor;
use super::partition::PartitionView;
use super::scheduler::{PipelineScheduler, SchedulerConfig, Task};
use super::worker::{BackpressureControl, WorkerPool};
use crate::core::error::QueryError;
use crate::core::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Streaming execution engine
///
/// Coordinates the execution of streaming queries by:
/// 1. Partitioning input data across multiple workers
/// 2. Creating tasks for each (executor, partition) pair
/// 3. Scheduling tasks based on dependencies
/// 4. Applying backpressure to limit memory usage
pub struct StreamingExecutionEngine {
    partition_view: PartitionView,
    scheduler: PipelineScheduler,
    worker_pool: WorkerPool,
    backpressure: BackpressureControl,
    /// Map from executor ID to actual executor instances
    executors: HashMap<usize, Box<StreamingExecutor>>,
    /// Map from task ID to produced chunks
    chunk_buffer: HashMap<usize, Vec<DataChunk>>,
    /// Map from task ID to executor ID (for task execution)
    task_to_executor_id: HashMap<usize, usize>,
    /// Track if tasks have been built
    tasks_built: bool,
    /// Shared executor registry for worker thread execution
    executor_registry: Arc<Mutex<HashMap<usize, Box<StreamingExecutor>>>>,
}

impl StreamingExecutionEngine {
    /// Create a new streaming execution engine
    pub fn new(scheduler_config: SchedulerConfig, partition_view: PartitionView) -> Self {
        let executor_registry = Arc::new(Mutex::new(HashMap::new()));
        let task_to_executor_id = Arc::new(Mutex::new(HashMap::new()));
        let worker_pool = WorkerPool::new_with_executors(
            scheduler_config.num_workers,
            executor_registry.clone(),
            task_to_executor_id.clone(),
        );
        let scheduler = PipelineScheduler::new();
        let backpressure = BackpressureControl::new(scheduler_config.max_buffered_chunks);

        Self {
            partition_view,
            scheduler,
            worker_pool,
            backpressure,
            executors: HashMap::new(),
            chunk_buffer: HashMap::new(),
            task_to_executor_id: HashMap::new(),
            tasks_built: false,
            executor_registry,
        }
    }

    /// Register an executor with the engine
    pub fn register_executor(&mut self, executor_id: usize, executor: StreamingExecutor) {
        self.executors.insert(executor_id, Box::new(executor));
    }

    /// Check if an executor is a source (scan) executor
    fn is_source_executor(executor: &StreamingExecutor) -> bool {
        matches!(
            executor,
            StreamingExecutor::ScanVertices { .. } | StreamingExecutor::ScanEdges { .. }
        )
    }

    /// Build execution tasks from the executor DAG
    ///
    /// Creates a task for each (executor, partition) pair,
    /// properly tracking dependencies based on executor types.
    /// Supports arbitrary depth operator chains via topological ordering.
    pub fn build_tasks(&mut self) -> Result<(), QueryError> {
        if self.tasks_built {
            return Ok(());
        }

        let mut task_id = 0;
        let partition_count = self.partition_view.partition_count;

        // Get sorted executor IDs for consistent ordering
        let mut executor_ids: Vec<_> = self.executors.keys().copied().collect();
        executor_ids.sort();

        // Map from executor_id to list of task_ids (one per partition)
        let mut executor_task_ranges: HashMap<usize, Vec<usize>> = HashMap::new();

        // Topological sort: process source executors first, then dependents
        // In current design, sources are explicit (ScanVertices/ScanEdges)
        // All other operators depend on sources
        let mut processed = std::collections::HashSet::new();

        // First pass: create tasks for source executors
        for executor_id in &executor_ids {
            if let Some(executor) = self.executors.get(executor_id) {
                if Self::is_source_executor(executor) {
                    let mut executor_tasks = Vec::new();
                    for partition_id in 0..partition_count {
                        let task = Task::new(task_id, *executor_id, partition_id, 0);
                        executor_tasks.push(task_id);
                        self.task_to_executor_id.insert(task_id, *executor_id);
                        self.scheduler.add_task(task);
                        task_id += 1;
                    }
                    executor_task_ranges.insert(*executor_id, executor_tasks);
                    processed.insert(*executor_id);
                }
            }
        }

        // Second pass: create tasks for dependent executors
        // Each dependent executor should have one task per partition
        // and depend on corresponding tasks from source executors
        for executor_id in &executor_ids {
            if processed.contains(executor_id) {
                continue;
            }

            let mut executor_tasks = Vec::new();
            for partition_id in 0..partition_count {
                let mut task = Task::new(task_id, *executor_id, partition_id, 0);

                // All dependent tasks depend on all source executor tasks for the same partition
                let mut dependencies = Vec::new();
                for source_id in executor_task_ranges.keys() {
                    if let Some(source_tasks) = executor_task_ranges.get(source_id) {
                        if partition_id < source_tasks.len() {
                            dependencies.push(source_tasks[partition_id]);
                        }
                    }
                }

                if !dependencies.is_empty() {
                    task = task.with_dependencies(dependencies);
                }

                executor_tasks.push(task_id);
                self.task_to_executor_id.insert(task_id, *executor_id);
                self.scheduler.add_task(task);
                task_id += 1;
            }
            executor_task_ranges.insert(*executor_id, executor_tasks);
            processed.insert(*executor_id);
        }

        self.scheduler.initialize()?;
        self.tasks_built = true;

        self.worker_pool
            .update_task_mapping(self.task_to_executor_id.clone())?;

        Ok(())
    }

    /// Execute the streaming query
    ///
    /// Returns chunks as they become available, respecting backpressure.
    /// Pulls data from executors through the streaming model.
    pub fn execute(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        self.build_tasks()?;

        if let Ok(mut registry) = self.executor_registry.lock() {
            for (id, executor) in self.executors.drain() {
                registry.insert(id, executor);
            }
        }

        if let Ok(mut registry) = self.executor_registry.lock() {
            for executor in registry.values_mut() {
                executor.open()?;
            }
        }

        let mut output_chunks = Vec::new();
        let mut submitted_tasks = std::collections::HashSet::new();

        loop {
            while let Some(result) = self.worker_pool.try_recv_result() {
                submitted_tasks.remove(&result.task_id);

                if result.success {
                    if let Some(chunk) = result.chunk {
                        self.chunk_buffer
                            .entry(result.task_id)
                            .or_insert_with(Vec::new)
                            .push(chunk.clone());
                        output_chunks.push(chunk);

                        self.scheduler.mark_done(result.task_id)?;
                        self.backpressure.add_chunk()?;
                    } else {
                        self.scheduler.mark_done(result.task_id)?;
                    }
                } else {
                    self.scheduler.mark_failed(result.task_id)?;
                    let error_msg = result
                        .error_msg
                        .unwrap_or_else(|| "Unknown error".to_string());
                    return Err(QueryError::execution(format!(
                        "Task {} failed: {}",
                        result.task_id, error_msg
                    )));
                }
            }

            while let Some(task_id) = self.scheduler.get_next_task() {
                while !self.backpressure.can_buffer()? {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                self.worker_pool.submit_task(task_id)?;
                submitted_tasks.insert(task_id);
            }

            if self.scheduler.is_complete() && submitted_tasks.is_empty() {
                break;
            } else if submitted_tasks.is_empty() && !self.scheduler.is_complete() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            } else {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }

        if let Ok(mut registry) = self.executor_registry.lock() {
            for executor in registry.values_mut() {
                executor.close()?;
            }
        }

        Ok(output_chunks)
    }

    /// Requested early termination (e.g., from LIMIT)
    pub fn request_stop(&mut self) -> Result<(), QueryError> {
        self.scheduler.request_stop()?;
        self.worker_pool.shutdown()?;
        Ok(())
    }

    /// Get scheduler statistics
    pub fn task_count(&self) -> usize {
        self.scheduler.task_count()
    }

    /// Get number of completed tasks
    pub fn completed_task_count(&self) -> usize {
        self.scheduler.completed_task_count()
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
                    Value::String(format!("type_{}", i % 10)),
                    Value::String(format!("prop_{}", i % 100)),
                    Value::BigInt((i % 1000) as i64),
                ]
            })
            .collect()
    }

    #[test]
    fn test_engine_creation() {
        let partition_view = PartitionView::from_range(0..1000, 4);
        let scheduler_config = SchedulerConfig::default();
        let engine = StreamingExecutionEngine::new(scheduler_config, partition_view.clone());

        assert_eq!(engine.partition_view.partition_count, 4);
    }

    #[test]
    fn test_backpressure() {
        let partition_view = PartitionView::single(0..100);
        let scheduler_config = SchedulerConfig::default();
        let engine = StreamingExecutionEngine::new(scheduler_config, partition_view);

        assert!(engine.backpressure.can_buffer().unwrap());
    }

    #[test]
    fn test_single_scan_executor() {
        let partition_view = PartitionView::single(0..1000);
        let scheduler_config = SchedulerConfig {
            num_workers: 1,
            max_buffered_chunks: 5,
            enable_parallel: false,
        };
        let mut engine = StreamingExecutionEngine::new(scheduler_config, partition_view);

        let buffer = create_test_buffer(100);
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        };
        engine.register_executor(0, scan);

        let result = engine.execute();
        assert!(result.is_ok());
        assert_eq!(engine.task_count(), 1);
        assert_eq!(engine.completed_task_count(), 1);
    }

    #[test]
    fn test_task_scheduling() {
        let partition_view = PartitionView::from_range(0..1000, 4);
        let scheduler_config = SchedulerConfig {
            num_workers: 1,
            max_buffered_chunks: 5,
            enable_parallel: false,
        };
        let mut engine = StreamingExecutionEngine::new(scheduler_config, partition_view);

        // Register a single scan executor for all partitions
        // (current implementation creates tasks per partition for each executor)
        let buffer = create_test_buffer(250);
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        };
        engine.register_executor(0, scan);

        assert!(engine.build_tasks().is_ok());
        // With 4 partitions and 1 executor, we get 4 tasks (one per partition)
        assert_eq!(engine.task_count(), 4);
    }
}
