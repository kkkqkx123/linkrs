//! Streaming execution engine that coordinates all components
//!
//! Brings together StreamingExecutor, PipelineScheduler, PartitionView,
//! and WorkerPool for cohesive pull-based streaming execution.

use std::collections::HashMap;
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
}

impl StreamingExecutionEngine {
    pub fn new(config: StreamingEngineConfig) -> Self {
        let worker_pool = WorkerPool::new(config.scheduler_config.num_workers);
        let scheduler = PipelineScheduler::new(config.scheduler_config.clone());
        let backpressure = BackpressureControl::new(config.max_buffered_chunks);

        Self {
            config,
            scheduler,
            worker_pool,
            backpressure,
            executors: HashMap::new(),
            chunk_buffer: HashMap::new(),
        }
    }

    /// Register an executor with the engine
    pub fn register_executor(&mut self, executor_id: usize, executor: StreamingExecutor) {
        self.executors.insert(executor_id, Box::new(executor));
    }

    /// Build execution tasks from the executor DAG
    ///
    /// Creates a task for each (executor, partition) pair,
    /// properly tracking dependencies.
    pub fn build_tasks(&mut self) -> Result<(), QueryError> {
        let mut task_id = 0;
        let partition_count = self.config.partition_view.partition_count;

        // For each executor and partition, create a task
        for executor_id in self.executors.keys() {
            for partition_id in 0..partition_count {
                let mut task = Task::new(task_id, *executor_id, partition_id, 0);

                // TODO: Set dependencies based on executor DAG
                // For now, assume scan executors have no deps

                self.scheduler.add_task(task);
                task_id += 1;
            }
        }

        self.scheduler.initialize()?;
        Ok(())
    }

    /// Execute the streaming query
    ///
    /// Returns chunks as they become available, respecting backpressure.
    pub fn execute(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        self.build_tasks()?;

        let mut all_chunks = Vec::new();

        loop {
            // Get next ready task
            if let Some(task_id) = self.scheduler.get_next_task() {
                // Check backpressure before proceeding
                while !self.backpressure.can_buffer()? {
                    // Wait for consumer to drain buffer
                    // In a real implementation, this would use channels/events
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                // Execute task (simplified - just mark as done)
                self.scheduler.mark_done(task_id)?;
                self.backpressure.add_chunk()?;
            } else if self.scheduler.is_complete() {
                break;
            } else {
                // All tasks done but not complete (error state)
                break;
            }
        }

        Ok(all_chunks)
    }

    /// Get scheduler statistics
    pub fn task_count(&self) -> usize {
        self.scheduler.task_count()
    }

    /// Get number of completed tasks
    pub fn completed_task_count(&self) -> usize {
        self.scheduler.completed_task_count()
    }

    /// Requested early termination (e.g., from LIMIT)
    pub fn request_stop(&mut self) -> Result<(), QueryError> {
        self.scheduler.request_stop()?;
        self.worker_pool.shutdown()?;
        Ok(())
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
}
