//! Worker thread pool for parallel task execution
//!
//! Provides a thread pool implementation for executing streaming tasks
//! in parallel, with backpressure and resource management.

use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;
use crate::core::error::QueryError;

/// Message sent to a worker thread
#[derive(Debug)]
pub enum WorkerMessage {
    /// Execute a task
    ExecuteTask(usize),
    /// Stop the worker
    Shutdown,
}

/// Result of task execution
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: usize,
    pub success: bool,
    pub error_msg: Option<String>,
}

/// Worker pool for parallel task execution
pub struct WorkerPool {
    /// Number of worker threads
    num_workers: usize,
    /// Channels for sending work to workers
    worker_senders: Vec<Sender<WorkerMessage>>,
    /// Receiver for task results
    result_receiver: Receiver<TaskResult>,
    /// Worker thread handles (kept alive)
    #[allow(dead_code)]
    worker_handles: Vec<thread::JoinHandle<()>>,
}

impl WorkerPool {
    /// Create a new worker pool with N workers
    pub fn new(num_workers: usize) -> Self {
        let (result_sender, result_receiver) = channel();
        let mut worker_senders = Vec::new();
        let mut worker_handles = Vec::new();

        for worker_id in 0..num_workers {
            let (tx, rx) = channel();
            worker_senders.push(tx);

            let result_sender = result_sender.clone();
            let handle = thread::spawn(move || {
                Self::worker_loop(worker_id, rx, result_sender);
            });

            worker_handles.push(handle);
        }

        Self {
            num_workers,
            worker_senders,
            result_receiver,
            worker_handles,
        }
    }

    /// Worker loop (runs in a thread)
    fn worker_loop(
        _worker_id: usize,
        rx: std::sync::mpsc::Receiver<WorkerMessage>,
        result_sender: Sender<TaskResult>,
    ) {
        while let Ok(msg) = rx.recv() {
            match msg {
                WorkerMessage::ExecuteTask(task_id) => {
                    // TODO: Actual task execution logic
                    // For now, just send success
                    let result = TaskResult {
                        task_id,
                        success: true,
                        error_msg: None,
                    };
                    let _ = result_sender.send(result);
                }
                WorkerMessage::Shutdown => {
                    break;
                }
            }
        }
    }

    /// Submit a task to a worker (round-robin)
    pub fn submit_task(&self, task_id: usize) -> Result<(), QueryError> {
        let worker_idx = task_id % self.num_workers;
        if let Some(sender) = self.worker_senders.get(worker_idx) {
            sender
                .send(WorkerMessage::ExecuteTask(task_id))
                .map_err(|e| QueryError::execution(format!("Failed to submit task: {}", e)))?;
            Ok(())
        } else {
            Err(QueryError::execution("No available workers"))
        }
    }

    /// Get next task result (non-blocking)
    pub fn try_recv_result(&self) -> Option<TaskResult> {
        self.result_receiver.try_recv().ok()
    }

    /// Shutdown the worker pool
    pub fn shutdown(&self) -> Result<(), QueryError> {
        for sender in &self.worker_senders {
            sender
                .send(WorkerMessage::Shutdown)
                .map_err(|e| QueryError::execution(format!("Failed to shutdown worker: {}", e)))?;
        }
        Ok(())
    }

    pub fn num_workers(&self) -> usize {
        self.num_workers
    }
}

/// Backpressure controller for pipeline execution
///
/// Limits the number of buffered chunks to prevent
/// excessive memory usage when producer is faster
/// than consumer.
#[derive(Debug, Clone)]
pub struct BackpressureControl {
    /// Maximum buffered chunks
    max_buffered: usize,
    /// Current buffered chunks
    current_buffered: Arc<Mutex<usize>>,
}

impl BackpressureControl {
    pub fn new(max_buffered: usize) -> Self {
        Self {
            max_buffered,
            current_buffered: Arc::new(Mutex::new(0)),
        }
    }

    /// Check if we can buffer more chunks
    pub fn can_buffer(&self) -> Result<bool, QueryError> {
        let buffered = self
            .current_buffered
            .lock()
            .map_err(|e| QueryError::execution(format!("Lock error: {}", e)))?;
        Ok(*buffered < self.max_buffered)
    }

    /// Add a buffered chunk
    pub fn add_chunk(&self) -> Result<(), QueryError> {
        let mut buffered = self
            .current_buffered
            .lock()
            .map_err(|e| QueryError::execution(format!("Lock error: {}", e)))?;
        if *buffered >= self.max_buffered {
            return Err(QueryError::execution("Buffer full"));
        }
        *buffered += 1;
        Ok(())
    }

    /// Remove a buffered chunk
    pub fn remove_chunk(&self) -> Result<(), QueryError> {
        let mut buffered = self
            .current_buffered
            .lock()
            .map_err(|e| QueryError::execution(format!("Lock error: {}", e)))?;
        if *buffered > 0 {
            *buffered -= 1;
        }
        Ok(())
    }

    /// Current buffered chunks
    pub fn buffered_count(&self) -> Result<usize, QueryError> {
        self.current_buffered
            .lock()
            .map(|b| *b)
            .map_err(|e| QueryError::execution(format!("Lock error: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_pool_creation() {
        let pool = WorkerPool::new(4);
        assert_eq!(pool.num_workers(), 4);
    }

    #[test]
    fn test_backpressure_control() {
        let bp = BackpressureControl::new(10);
        assert!(bp.can_buffer().unwrap());

        // Fill the buffer
        for _ in 0..10 {
            bp.add_chunk().unwrap();
        }

        assert!(!bp.can_buffer().unwrap());
        assert_eq!(bp.buffered_count().unwrap(), 10);

        // Remove one chunk
        bp.remove_chunk().unwrap();
        assert!(bp.can_buffer().unwrap());
        assert_eq!(bp.buffered_count().unwrap(), 9);
    }
}
