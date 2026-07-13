//! Morsel-style query-level bounded worker pool for dynamic partition execution.
//!
//! Unlike the per-Gather `ParallelPartitionCoordinator` which spawns threads per
//! Gather node with static round-robin partition assignment, this pool creates
//! workers once per query. Partition tasks are dynamically claimed from a shared
//! atomic counter — a "morsel queue" — providing natural load balancing: faster
//! workers automatically process more partitions.
//!
//! Phase 4: shared worker pool + morsel queue replaces per-Gather `thread::spawn`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::core::error::QueryError;

use super::chunk::DataChunk;
use super::coordinator::{release_queue_metrics, run_partition, PartitionMessage};
use super::executor::StreamingExecutor;
use super::runtime::ExecutionRuntime;

const CHANNEL_WAIT: Duration = Duration::from_millis(2);

/// A batch of partition tasks submitted to the pool for execution.
///
/// Workers dynamically claim partition indices via `next_index` and execute
/// the corresponding tree, sending output chunks through `senders[partition_id]`.
/// Partition trees are wrapped in `Mutex` to satisfy `Send + Sync` for `Arc<PartitionBatch>`
/// (each index is claimed by at most one worker via the atomic counter, so contention
/// on the mutex is limited to the brief `take()` window).
pub(crate) struct PartitionBatch {
    pub partitions: Vec<Mutex<Option<StreamingExecutor>>>,
    pub senders: Vec<SyncSender<PartitionMessage>>,
    pub error_tx: Sender<(usize, QueryError)>,
    pub next_index: AtomicUsize,
    pub total: usize,
    pub runtime: Arc<ExecutionRuntime>,
    pub worker_time_us: AtomicU64,
    pub buffered_chunks: Arc<AtomicUsize>,
    pub buffered_bytes: Arc<AtomicUsize>,
    pub buffered_chunks_peak: Arc<AtomicUsize>,
    pub buffered_bytes_peak: Arc<AtomicUsize>,
    pub stop: Arc<AtomicBool>,
}

impl PartitionBatch {
    /// Create a new batch and return it together with per-partition receivers
    /// and an error receiver.
    pub(crate) fn new(
        partitions: Vec<StreamingExecutor>,
        runtime: Arc<ExecutionRuntime>,
        max_buffered_chunks: usize,
    ) -> (Self, Vec<Receiver<PartitionMessage>>, Receiver<(usize, QueryError)>) {
        let partition_count = partitions.len();
        let capacity = max_buffered_chunks.max(1);
        let mut senders = Vec::with_capacity(partition_count);
        let mut receivers = Vec::with_capacity(partition_count);
        for _ in 0..partition_count {
            let (tx, rx) = mpsc::sync_channel(capacity);
            senders.push(tx);
            receivers.push(rx);
        }
        let (error_tx, error_rx) = mpsc::channel();

        let batch = Self {
            partitions: partitions.into_iter().map(|p| Mutex::new(Some(p))).collect(),
            senders,
            error_tx,
            next_index: AtomicUsize::new(0),
            total: partition_count,
            runtime,
            worker_time_us: AtomicU64::new(0),
            buffered_chunks: Arc::new(AtomicUsize::new(0)),
            buffered_bytes: Arc::new(AtomicUsize::new(0)),
            buffered_chunks_peak: Arc::new(AtomicUsize::new(0)),
            buffered_bytes_peak: Arc::new(AtomicUsize::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        (batch, receivers, error_rx)
    }
}

/// Handle for consuming results from a submitted batch.
///
/// Returned by `MorselWorkerPool::submit()`. Provides per-partition chunk
/// access (same interface as `ParallelPartitionCoordinator`).
#[derive(Debug)]
pub(crate) struct PartitionHandle {
    pub receivers: Vec<Receiver<PartitionMessage>>,
    pub error_rx: Mutex<Receiver<(usize, QueryError)>>,
    pub stop: Arc<AtomicBool>,
    pub partition_count: usize,
    pub buffered_chunks: Arc<AtomicUsize>,
    pub buffered_bytes: Arc<AtomicUsize>,
    pub buffered_chunks_peak: Arc<AtomicUsize>,
    pub buffered_bytes_peak: Arc<AtomicUsize>,
    pub worker_time_us: Arc<AtomicU64>,
    pub started_at: Instant,
    pub profile_recorded: bool,
    pub runtime: Arc<ExecutionRuntime>,
}

impl PartitionHandle {
    /// Create a handle from a batch and its associated receivers.
    pub(crate) fn from_batch(
        batch: &Arc<PartitionBatch>,
        receivers: Vec<Receiver<PartitionMessage>>,
        error_rx: Receiver<(usize, QueryError)>,
        runtime: Arc<ExecutionRuntime>,
        started_at: Instant,
    ) -> Self {
        Self {
            receivers,
            error_rx: Mutex::new(error_rx),
            stop: batch.stop.clone(),
            partition_count: batch.total,
            buffered_chunks: batch.buffered_chunks.clone(),
            buffered_bytes: batch.buffered_bytes.clone(),
            buffered_chunks_peak: batch.buffered_chunks_peak.clone(),
            buffered_bytes_peak: batch.buffered_bytes_peak.clone(),
            worker_time_us: Arc::new(AtomicU64::new(0)),
            started_at,
            profile_recorded: false,
            runtime,
        }
    }

    /// Pull one chunk from an individual partition.
    pub fn next_for_partition(
        &mut self,
        partition_id: usize,
    ) -> Result<Option<DataChunk>, QueryError> {
        if partition_id >= self.receivers.len() {
            return Err(QueryError::execution(format!(
                "Morsel partition handle has no partition {partition_id}",
            )));
        }

        loop {
            self.check_worker_error()?;
            if self.runtime.is_cancelled() {
                let _ = self.stop_and_join();
                return Err(QueryError::execution("Query cancelled".to_string()));
            }
            match self.receivers[partition_id].recv_timeout(CHANNEL_WAIT) {
                Ok(PartitionMessage::Chunk(buffered)) => {
                    release_queue_metrics(
                        &self.buffered_chunks,
                        &self.buffered_bytes,
                        buffered.bytes,
                    );
                    return Ok(Some(buffered.chunk));
                }
                Ok(PartitionMessage::Finished) => {
                    return Ok(None);
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    self.check_worker_error()?;
                    let _ = self.stop_and_join();
                    return Err(QueryError::execution(format!(
                        "Morsel partition {partition_id} disconnected before completion",
                    )));
                }
            }
        }
    }

    pub fn stop_and_join(&mut self) -> Result<(), QueryError> {
        self.stop.store(true, Ordering::Relaxed);
        self.record_profile();
        Ok(())
    }

    fn check_worker_error(&mut self) -> Result<(), QueryError> {
        let error = { self.error_rx.lock().unwrap().try_recv() };
        match error {
            Ok((partition_id, error)) => {
                let _ = self.stop_and_join();
                Err(QueryError::execution(format!(
                    "Morsel partition {partition_id} failed: {error}",
                )))
            }
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => Ok(()),
        }
    }

    fn record_profile(&mut self) {
        if self.profile_recorded {
            return;
        }
        self.profile_recorded = true;
        let mut profile = self.runtime.profile().lock();
        profile.parallel_wall_time_us = profile
            .parallel_wall_time_us
            .saturating_add(self.started_at.elapsed().as_micros() as u64);
        profile.parallel_work_time_us = profile
            .parallel_work_time_us
            .saturating_add(self.worker_time_us.load(Ordering::Relaxed));
        profile.parallel_buffered_chunks_peak = profile
            .parallel_buffered_chunks_peak
            .max(self.buffered_chunks_peak.load(Ordering::Relaxed));
        profile.parallel_buffered_bytes_peak = profile
            .parallel_buffered_bytes_peak
            .max(self.buffered_bytes_peak.load(Ordering::Relaxed));
    }
}

/// Query-level bounded worker pool with morsel-style dynamic task assignment.
///
/// Workers are created once per query and persist across multiple Exchange
/// node invocations. Partition tasks are dynamically claimed from a shared
/// atomic counter, so faster workers naturally process more work — this
/// eliminates the static round-robin load imbalance of per-Gather threads.
pub struct MorselWorkerPool {
    batch_tx: Sender<Arc<PartitionBatch>>,
    workers: Vec<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    max_workers: usize,
}

impl MorselWorkerPool {
    /// Create a new pool with `max_workers` persistent worker threads.
    pub fn new(max_workers: usize) -> Self {
        let (batch_tx, batch_rx) = mpsc::channel::<Arc<PartitionBatch>>();
        let batch_rx = Arc::new(Mutex::new(batch_rx));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_count = max_workers.max(1);
        let mut workers = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let rx = batch_rx.clone();
            let stopper = stop.clone();
            workers.push(thread::spawn(move || loop {
                if stopper.load(Ordering::Relaxed) {
                    return;
                }
                let batch = {
                    let guard = rx.lock().unwrap();
                    guard.recv()
                };
                match batch {
                    Ok(batch) => Self::process_batch(batch, &stopper),
                    Err(_) => return,
                }
            }));
        }

        Self {
            batch_tx,
            workers,
            stop,
            max_workers: worker_count,
        }
    }

    /// Submit a batch of partition tasks for morsel-style execution.
    ///
    /// Workers dynamically claim partitions from the batch's shared atomic
    /// counter.
    pub(crate) fn submit(&self, batch: Arc<PartitionBatch>) {
        let workers_to_notify = self.max_workers.min(batch.total);
        for _ in 0..workers_to_notify {
            if self.batch_tx.send(batch.clone()).is_err() {
                break;
            }
        }
    }

    /// Number of workers in this pool.
    pub fn max_workers(&self) -> usize {
        self.max_workers
    }

    /// Signal all workers to stop and join them. Called during query teardown.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }

    fn process_batch(batch: Arc<PartitionBatch>, stopper: &AtomicBool) {
        loop {
            if stopper.load(Ordering::Relaxed) || batch.runtime.is_cancelled() {
                return;
            }
            let index = batch.next_index.fetch_add(1, Ordering::Relaxed);
            if index >= batch.total {
                return;
            }

            let mut tree = batch.partitions[index].lock().unwrap().take();
            let Some(ref mut tree) = tree else {
                continue;
            };

            let started = Instant::now();
            let result = run_partition(
                tree,
                &batch.senders[index],
                &batch.stop,
                &batch.runtime,
                &batch.buffered_chunks,
                &batch.buffered_chunks_peak,
                &batch.buffered_bytes,
                &batch.buffered_bytes_peak,
            );
            batch
                .worker_time_us
                .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);

            if let Err(error) = result {
                batch.stop.store(true, Ordering::Relaxed);
                let _ = batch.error_tx.send((index, error));
                return;
            }
        }
    }
}

impl Drop for MorselWorkerPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for MorselWorkerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MorselWorkerPool")
            .field("max_workers", &self.max_workers)
            .field("worker_count", &self.workers.len())
            .finish()
    }
}
