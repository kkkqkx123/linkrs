//! Pipeline scheduler for streaming execution
//!
//! Coordinates execution of streaming tasks with support for:
//! - Pull-based execution (consumer-driven)
//! - Backpressure handling
//! - Early termination (LIMIT)
//! - Parallel task execution

use std::collections::VecDeque;
use crate::core::error::QueryError;

/// Status of a pipeline task
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// Waiting for inputs or execution
    Pending,
    /// Currently executing
    Running,
    /// Completed successfully
    Done,
    /// Stopped (e.g., by LIMIT)
    Stopped,
    /// Error occurred
    Error,
}

/// A single task in the execution pipeline
///
/// Each task represents the execution of one operator
/// on one partition of data (one chunk).
#[derive(Debug, Clone)]
pub struct Task {
    /// Unique task ID
    pub task_id: usize,
    /// ID of the executor node
    pub executor_id: usize,
    /// Partition ID (for parallel execution)
    pub partition_id: usize,
    /// Chunk index (which chunk to process)
    pub chunk_index: usize,
    /// Current status
    pub status: TaskStatus,
    /// IDs of tasks that must complete before this one
    pub dependencies: Vec<usize>,
}

impl Task {
    pub fn new(
        task_id: usize,
        executor_id: usize,
        partition_id: usize,
        chunk_index: usize,
    ) -> Self {
        Self {
            task_id,
            executor_id,
            partition_id,
            chunk_index,
            status: TaskStatus::Pending,
            dependencies: vec![],
        }
    }

    pub fn with_dependencies(mut self, deps: Vec<usize>) -> Self {
        self.dependencies = deps;
        self
    }
}

/// Configuration for pipeline scheduler
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Number of worker threads
    pub num_workers: usize,
    /// Maximum buffered chunks per task
    pub max_buffered_chunks: usize,
    /// Whether to enable parallel execution
    pub enable_parallel: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            num_workers: 4,  // Default to 4 workers
            max_buffered_chunks: 10,
            enable_parallel: true,
        }
    }
}

/// Pipeline scheduler for coordinating streaming executor tasks
///
/// Uses pull-based execution model where consumer pulls from producer.
/// Supports:
/// - Task dependency resolution
/// - Backpressure (buffering limits)
/// - Early termination signals
pub struct PipelineScheduler {
    /// Configuration
    config: SchedulerConfig,
    /// All tasks in the execution plan
    tasks: Vec<Task>,
    /// Queue of ready tasks (all dependencies satisfied)
    ready_queue: VecDeque<usize>,
    /// Execution stats
    stats: ExecutionStats,
}

#[derive(Debug, Default)]
pub struct ExecutionStats {
    pub tasks_completed: usize,
    pub tasks_failed: usize,
    pub chunks_processed: usize,
}

impl PipelineScheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        Self {
            config,
            tasks: Vec::new(),
            ready_queue: VecDeque::new(),
            stats: ExecutionStats::default(),
        }
    }

    pub fn default_config() -> Self {
        Self::new(SchedulerConfig::default())
    }

    /// Add a task to the scheduler
    pub fn add_task(&mut self, task: Task) {
        self.tasks.push(task);
    }

    /// Check if a task's dependencies are satisfied
    fn is_ready(&self, task_id: usize) -> bool {
        if let Some(task) = self.tasks.get(task_id) {
            task.dependencies.iter().all(|dep_id| {
                self.tasks
                    .get(*dep_id)
                    .map(|t| t.status == TaskStatus::Done)
                    .unwrap_or(false)
            })
        } else {
            false
        }
    }

    /// Initialize the scheduler by finding initially ready tasks
    pub fn initialize(&mut self) -> Result<(), QueryError> {
        // Find all tasks with no dependencies
        for (id, task) in self.tasks.iter().enumerate() {
            if task.dependencies.is_empty() {
                self.ready_queue.push_back(id);
            }
        }
        Ok(())
    }

    /// Get the next ready task
    ///
    /// Returns a task that is ready to execute (all dependencies satisfied).
    pub fn get_next_task(&mut self) -> Option<usize> {
        while let Some(task_id) = self.ready_queue.pop_front() {
            let is_ready = {
                if let Some(task) = self.tasks.get(task_id) {
                    task.status == TaskStatus::Pending && self.is_ready(task_id)
                } else {
                    false
                }
            };

            if is_ready {
                if let Some(task) = self.tasks.get_mut(task_id) {
                    task.status = TaskStatus::Running;
                    return Some(task_id);
                }
            }
        }
        None
    }

    /// Mark a task as completed
    pub fn mark_done(&mut self, task_id: usize) -> Result<(), QueryError> {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = TaskStatus::Done;
            self.stats.tasks_completed += 1;

            // Add newly ready tasks to the queue
            for (id, task) in self.tasks.iter().enumerate() {
                if task.status == TaskStatus::Pending && self.is_ready(id) {
                    self.ready_queue.push_back(id);
                }
            }

            Ok(())
        } else {
            Err(QueryError::execution(format!("Task {} not found", task_id)))
        }
    }

    /// Mark a task as failed
    pub fn mark_failed(&mut self, task_id: usize) -> Result<(), QueryError> {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = TaskStatus::Error;
            self.stats.tasks_failed += 1;
            Ok(())
        } else {
            Err(QueryError::execution(format!("Task {} not found", task_id)))
        }
    }

    /// Signal early termination (e.g., from LIMIT)
    pub fn request_stop(&mut self) -> Result<(), QueryError> {
        for task in &mut self.tasks {
            if task.status == TaskStatus::Pending || task.status == TaskStatus::Running {
                task.status = TaskStatus::Stopped;
            }
        }
        Ok(())
    }

    /// Check if all tasks are completed
    pub fn is_complete(&self) -> bool {
        self.tasks.iter().all(|t| {
            t.status == TaskStatus::Done || t.status == TaskStatus::Error || t.status == TaskStatus::Stopped
        })
    }

    /// Get execution statistics
    pub fn stats(&self) -> &ExecutionStats {
        &self.stats
    }

    /// Total number of tasks
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Number of completed tasks
    pub fn completed_task_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Done)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_creation() {
        let task = Task::new(0, 1, 0, 0);
        assert_eq!(task.task_id, 0);
        assert_eq!(task.executor_id, 1);
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn test_scheduler_initialization() {
        let config = SchedulerConfig::default();
        let mut scheduler = PipelineScheduler::new(config);

        // Add independent tasks
        scheduler.add_task(Task::new(0, 0, 0, 0));
        scheduler.add_task(Task::new(1, 0, 1, 0));

        scheduler.initialize().unwrap();
        assert_eq!(scheduler.ready_queue.len(), 2);
    }

    #[test]
    fn test_task_dependencies() {
        let config = SchedulerConfig::default();
        let mut scheduler = PipelineScheduler::new(config);

        // Task 0 has no dependencies
        scheduler.add_task(Task::new(0, 0, 0, 0));
        // Task 1 depends on Task 0
        scheduler.add_task(Task::new(1, 1, 0, 0).with_dependencies(vec![0]));

        scheduler.initialize().unwrap();

        // Only task 0 should be ready initially
        assert_eq!(scheduler.ready_queue.len(), 1);

        // Execute task 0
        let task_id = scheduler.get_next_task().unwrap();
        assert_eq!(task_id, 0);
        scheduler.mark_done(0).unwrap();

        // Now task 1 should be ready
        assert_eq!(scheduler.ready_queue.len(), 1);
        let task_id = scheduler.get_next_task().unwrap();
        assert_eq!(task_id, 1);
    }

    #[test]
    fn test_scheduler_completion() {
        let config = SchedulerConfig::default();
        let mut scheduler = PipelineScheduler::new(config);

        scheduler.add_task(Task::new(0, 0, 0, 0));
        scheduler.add_task(Task::new(1, 1, 0, 0).with_dependencies(vec![0]));

        scheduler.initialize().unwrap();

        // Process all tasks
        while let Some(task_id) = scheduler.get_next_task() {
            scheduler.mark_done(task_id).unwrap();
        }

        assert!(scheduler.is_complete());
        assert_eq!(scheduler.completed_task_count(), 2);
    }
}
