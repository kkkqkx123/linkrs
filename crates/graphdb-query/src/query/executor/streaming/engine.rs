//! Streaming execution engine that coordinates all components
//!
//! Brings together StreamingExecutor, PipelineScheduler, PartitionView,
//! and WorkerPool for cohesive pull-based streaming execution.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::core::error::QueryError;
use super::chunk::DataChunk;
use super::executor::StreamingExecutor;
use super::partition::PartitionView;
use super::scheduler::{PipelineScheduler, SchedulerConfig, Task};
use super::worker::{WorkerPool, BackpressureControl};

/// Configuration for the streaming execution engine
#[derive(Debug, Clone)]
pub struct StreamingEngineConfig {
    /// Scheduler configuration
    pub scheduler_config: SchedulerConfig,
    /// Partition configuration
    pub partition_view: PartitionView,
    /// Backpressure configuration
    pub max_buffered_chunks: usize,
}

impl StreamingEngineConfig {
    pub fn new(scheduler_config: SchedulerConfig, partition_view: PartitionView) -> Self {
        Self {
            scheduler_config: scheduler_config.clone(),
            partition_view,
            max_buffered_chunks: scheduler_config.max_buffered_chunks,
        }
    }
}

/// Streaming execution engine
///
/// Coordinates the execution of streaming queries by:
/// 1. Partitioning input data across multiple workers
/// 2. Creating tasks for each (executor, partition) pair
/// 3. Scheduling tasks based on dependencies
/// 4. Applying backpressure to limit memory usage
pub struct StreamingExecutionEngine {
    config: StreamingEngineConfig,
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
    /// Shared executor registry for Phase 3 worker thread execution
    executor_registry: Arc<Mutex<HashMap<usize, Box<StreamingExecutor>>>>,
}

impl StreamingExecutionEngine {
    pub fn new(config: StreamingEngineConfig) -> Self {
        let executor_registry = Arc::new(Mutex::new(HashMap::new()));
        let task_to_executor_id = Arc::new(Mutex::new(HashMap::new()));
        let worker_pool = WorkerPool::new_with_executors(
            config.scheduler_config.num_workers,
            executor_registry.clone(),
            task_to_executor_id.clone(),
        );
        let scheduler = PipelineScheduler::new(config.scheduler_config.clone());
        let backpressure = BackpressureControl::new(config.max_buffered_chunks);

        Self {
            config,
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
    pub fn build_tasks(&mut self) -> Result<(), QueryError> {
        // Only build tasks once
        if self.tasks_built {
            return Ok(());
        }

        let mut task_id = 0;
        let partition_count = self.config.partition_view.partition_count;

        // Get sorted executor IDs to ensure consistent ordering
        let mut executor_ids: Vec<_> = self.executors.keys().copied().collect();
        executor_ids.sort();

        // First pass: identify source and dependent executors
        let mut source_executor_ids = Vec::new();
        let mut dependent_executor_ids = Vec::new();

        for executor_id in &executor_ids {
            if let Some(executor) = self.executors.get(executor_id) {
                if Self::is_source_executor(executor) {
                    source_executor_ids.push(*executor_id);
                } else {
                    dependent_executor_ids.push(*executor_id);
                }
            }
        }

        // Create tasks for source executors (no dependencies)
        let mut source_task_ranges: HashMap<usize, Vec<usize>> = HashMap::new();
        for executor_id in source_executor_ids {
            let mut executor_tasks = Vec::new();
            for partition_id in 0..partition_count {
                let task = Task::new(task_id, executor_id, partition_id, 0);
                executor_tasks.push(task_id);
                self.task_to_executor_id.insert(task_id, executor_id);
                self.scheduler.add_task(task);
                task_id += 1;
            }
            source_task_ranges.insert(executor_id, executor_tasks);
        }

        // Create tasks for dependent executors (depend on all source executors)
        for executor_id in dependent_executor_ids {
            for partition_id in 0..partition_count {
                let mut task = Task::new(task_id, executor_id, partition_id, 0);

                // Collect all source tasks that produce data for this partition
                let mut dependencies = Vec::new();
                for source_id in source_task_ranges.keys() {
                    if let Some(source_tasks) = source_task_ranges.get(source_id) {
                        if partition_id < source_tasks.len() {
                            dependencies.push(source_tasks[partition_id]);
                        }
                    }
                }

                if !dependencies.is_empty() {
                    task = task.with_dependencies(dependencies);
                }

                self.task_to_executor_id.insert(task_id, executor_id);
                self.scheduler.add_task(task);
                task_id += 1;
            }
        }

        self.scheduler.initialize()?;
        self.tasks_built = true;

        // Phase 3: Update worker pool with task-to-executor mapping
        // This mapping is used by worker threads to find the executor for each task
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

        // Phase 3: Transfer executors to shared registry for worker thread access
        {
            if let Ok(mut registry) = self.executor_registry.lock() {
                // Move executors from local map to shared registry
                for (id, executor) in self.executors.drain() {
                    registry.insert(id, executor);
                }
            }
        }

        // Open all executors from the shared registry
        {
            if let Ok(mut registry) = self.executor_registry.lock() {
                for executor in registry.values_mut() {
                    executor.open()?;
                }
            }
        }

        let mut output_chunks = Vec::new();
        let mut submitted_tasks = std::collections::HashSet::new();

        loop {
            // Try to receive results from workers (non-blocking)
            while let Some(result) = self.worker_pool.try_recv_result() {
                submitted_tasks.remove(&result.task_id);

                if result.success {
                    if let Some(chunk) = result.chunk {
                        // Store the chunk
                        self.chunk_buffer
                            .entry(result.task_id)
                            .or_insert_with(Vec::new)
                            .push(chunk.clone());
                        output_chunks.push(chunk);

                        // Mark task as done
                        self.scheduler.mark_done(result.task_id)?;
                        self.backpressure.add_chunk()?;
                    } else {
                        // Executor returned None (no more data)
                        self.scheduler.mark_done(result.task_id)?;
                    }
                } else {
                    // Execution failed
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

            // Submit new tasks to workers
            while let Some(task_id) = self.scheduler.get_next_task() {
                // Check backpressure before submitting
                while !self.backpressure.can_buffer()? {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                // Phase 3: Submit to worker pool instead of direct execution
                self.worker_pool.submit_task(task_id)?;
                submitted_tasks.insert(task_id);
            }

            // Check if we're done
            if self.scheduler.is_complete() && submitted_tasks.is_empty() {
                break;
            } else if submitted_tasks.is_empty() && !self.scheduler.is_complete() {
                // No tasks submitted but not complete - wait for existing results
                std::thread::sleep(std::time::Duration::from_millis(10));
            } else {
                // Tasks are in flight, wait briefly before checking again
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }

        // Close all executors from the shared registry
        {
            if let Ok(mut registry) = self.executor_registry.lock() {
                for executor in registry.values_mut() {
                    executor.close()?;
                }
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

    #[test]
    fn test_engine_creation() {
        let partition_view = PartitionView::from_range(0..1000, 4);
        let scheduler_config = SchedulerConfig::default();
        let config = StreamingEngineConfig::new(scheduler_config, partition_view);
        let engine = StreamingExecutionEngine::new(config);

        assert_eq!(engine.config.partition_view.partition_count, 4);
    }

    #[test]
    fn test_backpressure() {
        let partition_view = PartitionView::single(0..100);
        let scheduler_config = SchedulerConfig::default();
        let config = StreamingEngineConfig::new(scheduler_config, partition_view);
        let engine = StreamingExecutionEngine::new(config);

        assert!(engine.backpressure.can_buffer().unwrap());
    }

    #[test]
    fn test_full_end_to_end_execution() {
        // Test complete execution flow with multiple partitions and executors
        let partition_view = PartitionView::from_range(0..2000, 2);
        let scheduler_config = SchedulerConfig {
            num_workers: 2,
            max_buffered_chunks: 5,
            enable_parallel: true,
        };
        let config = StreamingEngineConfig::new(scheduler_config, partition_view);
        let mut engine = StreamingExecutionEngine::new(config);

        // Register scan executors for each partition
        for i in 0..2 {
            let scan = StreamingExecutor::ScanVertices {
                partition_id: i,
                partition_range: (i as u32 * 1000)..(i as u32 * 1000 + 1000),
                current_offset: i as u32 * 1000,
            };
            engine.register_executor(i, scan);
        }

        // Execute should complete successfully
        let result = engine.execute();
        assert!(result.is_ok());

        // Verify execution statistics
        assert_eq!(engine.task_count(), 4); // 2 executors * 2 partitions
        assert_eq!(engine.completed_task_count(), 4);
    }

    #[test]
    fn test_scheduler_with_dependencies() {
        let partition_view = PartitionView::single(0..100);
        let scheduler_config = SchedulerConfig::default();
        let config = StreamingEngineConfig::new(scheduler_config, partition_view);
        let mut engine = StreamingExecutionEngine::new(config);

        // Create scan executor
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        };
        engine.register_executor(0, scan);

        // Build tasks and verify execution
        assert!(engine.build_tasks().is_ok());
        assert_eq!(engine.task_count(), 1);

        let result = engine.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_task_scheduling() {
        let partition_view = PartitionView::from_range(0..1000, 4);
        let scheduler_config = SchedulerConfig::default();
        let config = StreamingEngineConfig::new(scheduler_config, partition_view);
        let mut engine = StreamingExecutionEngine::new(config);

        // Register scan executors
        for i in 0..4 {
            let scan = StreamingExecutor::ScanVertices {
                partition_id: i,
                partition_range: (i as u32 * 250)..(i as u32 * 250 + 250),
                current_offset: i as u32 * 250,
            };
            engine.register_executor(i, scan);
        }

        // Verify task building
        assert!(engine.build_tasks().is_ok());
        // Should have 4 executors * 4 partitions = 16 tasks
        assert_eq!(engine.task_count(), 16);
    }
}
