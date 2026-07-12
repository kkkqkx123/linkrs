//! P8 bounded parallel coordinator for partition-local streaming trees.
//!
//! Workers own their executor trees and send bounded, memory-accounted chunks
//! to the Gather operator. This module deliberately has no raw pointers or
//! unsafe Send implementations.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::core::error::QueryError;
use crate::query::executor::base::{MemoryBudget, MemoryReservation};

use super::chunk::DataChunk;
use super::executor::StreamingExecutor;
use super::parallel_safety::is_parallel_safe;
use super::runtime::ExecutionRuntime;

const CHANNEL_WAIT: Duration = Duration::from_millis(2);

#[derive(Debug)]
struct BufferedChunk {
    chunk: DataChunk,
    _reservation: MemoryReservation,
    bytes: usize,
}

#[derive(Debug)]
enum PartitionMessage {
    Chunk(BufferedChunk),
    Finished,
}

/// Owns parallel workers below one formal Gather node.
#[derive(Debug)]
pub struct ParallelPartitionCoordinator {
    receivers: Vec<Receiver<PartitionMessage>>,
    completed: Vec<bool>,
    error_rx: Receiver<(usize, QueryError)>,
    stop: Arc<AtomicBool>,
    runtime: Arc<ExecutionRuntime>,
    workers: Vec<JoinHandle<()>>,
    started_at: Instant,
    worker_count: usize,
    worker_time_us: Arc<AtomicU64>,
    buffered_chunks: Arc<AtomicUsize>,
    buffered_chunks_peak: Arc<AtomicUsize>,
    buffered_bytes: Arc<AtomicUsize>,
    buffered_bytes_peak: Arc<AtomicUsize>,
    profile_recorded: bool,
}

impl ParallelPartitionCoordinator {
    /// Start worker threads for independent partition-local trees.
    pub fn start(
        partitions: Vec<StreamingExecutor>,
        runtime: Arc<ExecutionRuntime>,
        max_workers: usize,
        max_buffered_chunks: usize,
    ) -> Result<Self, QueryError> {
        if partitions.len() <= 1 || max_workers <= 1 {
            return Err(QueryError::execution(
                "Parallel coordinator requires at least two partitions and workers".to_string(),
            ));
        }
        if partitions.iter().any(|tree| !is_parallel_safe(tree)) {
            return Err(QueryError::execution(
                "Parallel coordinator received a non-parallel-safe tree".to_string(),
            ));
        }

        let partition_count = partitions.len();
        let worker_count = max_workers.min(partition_count);
        let capacity = max_buffered_chunks.max(1);
        let mut senders = Vec::with_capacity(partition_count);
        let mut receivers = Vec::with_capacity(partition_count);
        for _ in 0..partition_count {
            let (tx, rx) = mpsc::sync_channel(capacity);
            senders.push(tx);
            receivers.push(rx);
        }
        let (error_tx, error_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_time_us = Arc::new(AtomicU64::new(0));
        let buffered_chunks = Arc::new(AtomicUsize::new(0));
        let buffered_chunks_peak = Arc::new(AtomicUsize::new(0));
        let buffered_bytes = Arc::new(AtomicUsize::new(0));
        let buffered_bytes_peak = Arc::new(AtomicUsize::new(0));

        let mut groups: Vec<Vec<(usize, StreamingExecutor)>> =
            (0..worker_count).map(|_| Vec::new()).collect();
        for (partition_id, tree) in partitions.into_iter().enumerate() {
            groups[partition_id % worker_count].push((partition_id, tree));
        }

        let mut workers = Vec::with_capacity(worker_count);
        for group in groups {
            let worker_senders = senders.clone();
            let worker_error_tx = error_tx.clone();
            let worker_stop = stop.clone();
            let worker_runtime = runtime.clone();
            let worker_time = worker_time_us.clone();
            let queued_chunks = buffered_chunks.clone();
            let queued_chunks_peak = buffered_chunks_peak.clone();
            let queued_bytes = buffered_bytes.clone();
            let queued_bytes_peak = buffered_bytes_peak.clone();
            workers.push(thread::spawn(move || {
                for (partition_id, mut tree) in group {
                    if worker_stop.load(Ordering::Relaxed) || worker_runtime.is_cancelled() {
                        break;
                    }
                    let started = Instant::now();
                    let result = run_partition(
                        &mut tree,
                        &worker_senders[partition_id],
                        &worker_stop,
                        &worker_runtime,
                        &queued_chunks,
                        &queued_chunks_peak,
                        &queued_bytes,
                        &queued_bytes_peak,
                    );
                    worker_time.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                    if let Err(error) = result {
                        worker_stop.store(true, Ordering::Relaxed);
                        let _ = worker_error_tx.send((partition_id, error));
                        break;
                    }
                }
            }));
        }
        drop(error_tx);
        drop(senders);

        Ok(Self {
            receivers,
            completed: vec![false; partition_count],
            error_rx,
            stop,
            runtime,
            workers,
            started_at: Instant::now(),
            worker_count,
            worker_time_us,
            buffered_chunks,
            buffered_chunks_peak,
            buffered_bytes,
            buffered_bytes_peak,
            profile_recorded: false,
        })
    }

    /// Pull one chunk from an individual partition.
    ///
    /// Gather controls the call order, preserving concatenate order and the
    /// existing k-way merge semantics without materialising all partitions.
    pub fn next_for_partition(
        &mut self,
        partition_id: usize,
    ) -> Result<Option<DataChunk>, QueryError> {
        if partition_id >= self.receivers.len() {
            return Err(QueryError::execution(format!(
                "Parallel coordinator has no partition {partition_id}",
            )));
        }
        if self.completed[partition_id] {
            return Ok(None);
        }

        loop {
            self.check_worker_error()?;
            if let Err(error) = self.runtime.ensure_not_cancelled() {
                let _ = self.stop_and_join();
                return Err(error);
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
                    self.completed[partition_id] = true;
                    return Ok(None);
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    self.check_worker_error()?;
                    let _ = self.stop_and_join();
                    return Err(QueryError::execution(format!(
                        "Parallel partition {partition_id} stopped before signalling completion",
                    )));
                }
            }
        }
    }

    pub fn stop_and_join(&mut self) -> Result<(), QueryError> {
        self.stop.store(true, Ordering::Relaxed);
        let mut join_error = None;
        for worker in self.workers.drain(..) {
            if worker.join().is_err() && join_error.is_none() {
                join_error = Some(QueryError::execution(
                    "Parallel partition worker panicked".to_string(),
                ));
            }
        }
        self.record_profile();
        join_error.map_or(Ok(()), Err)
    }

    fn check_worker_error(&mut self) -> Result<(), QueryError> {
        match self.error_rx.try_recv() {
            Ok((partition_id, error)) => {
                // The producer that failed already requested a stop. Join all
                // workers before exposing the error so no worker can outlive
                // the failed streaming operation.
                let _ = self.stop_and_join();
                Err(QueryError::execution(format!(
                    "Parallel partition {partition_id} failed: {error}",
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
        profile.parallel_workers = profile.parallel_workers.max(self.worker_count);
        profile.parallel_buffered_chunks_peak = profile
            .parallel_buffered_chunks_peak
            .max(self.buffered_chunks_peak.load(Ordering::Relaxed));
        profile.parallel_buffered_bytes_peak = profile
            .parallel_buffered_bytes_peak
            .max(self.buffered_bytes_peak.load(Ordering::Relaxed));
    }
}

impl Drop for ParallelPartitionCoordinator {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_partition(
    tree: &mut StreamingExecutor,
    sender: &SyncSender<PartitionMessage>,
    stop: &AtomicBool,
    runtime: &ExecutionRuntime,
    queued_chunks: &AtomicUsize,
    queued_chunks_peak: &AtomicUsize,
    queued_bytes: &AtomicUsize,
    queued_bytes_peak: &AtomicUsize,
) -> Result<(), QueryError> {
    tree.open()?;
    let execution_result = (|| {
        while !stop.load(Ordering::Relaxed) {
            runtime.ensure_not_cancelled()?;
            let Some(chunk) = tree.advance()? else {
                send_message(sender, PartitionMessage::Finished, stop, runtime)?;
                return Ok(());
            };
            let bytes = MemoryBudget::estimate_rows_memory(&chunk.rows);
            let reservation = runtime.memory_budget.reserve(bytes)?;
            let message = PartitionMessage::Chunk(BufferedChunk {
                chunk,
                _reservation: reservation,
                bytes,
            });
            send_chunk(
                sender,
                message,
                stop,
                runtime,
                queued_chunks,
                queued_chunks_peak,
                queued_bytes,
                queued_bytes_peak,
            )?;
        }
        Ok(())
    })();
    let close_result = tree.close_tree();
    execution_result.and(close_result)
}

#[allow(clippy::too_many_arguments)]
fn send_chunk(
    sender: &SyncSender<PartitionMessage>,
    mut message: PartitionMessage,
    stop: &AtomicBool,
    runtime: &ExecutionRuntime,
    queued_chunks: &AtomicUsize,
    queued_chunks_peak: &AtomicUsize,
    queued_bytes: &AtomicUsize,
    queued_bytes_peak: &AtomicUsize,
) -> Result<(), QueryError> {
    loop {
        if stop.load(Ordering::Relaxed) || runtime.is_cancelled() {
            return Ok(());
        }
        let bytes = match &message {
            PartitionMessage::Chunk(buffered) => buffered.bytes,
            PartitionMessage::Finished => 0,
        };
        // Increment before publishing to prevent underflow: the receiver may
        // run as soon as `try_send` succeeds, so counting afterwards would
        // allow the receiver to decrement before this producer increments.
        let pre_count = queued_chunks.fetch_add(1, Ordering::Relaxed);
        let pre_bytes = queued_bytes.fetch_add(bytes, Ordering::Relaxed);
        match sender.try_send(message) {
            Ok(()) => {
                // Only record peak after a successful send, so transient
                // retries on a full channel don't inflate the peak.
                update_peak_value(queued_chunks_peak, pre_count + 1);
                update_peak_value(queued_bytes_peak, pre_bytes + bytes);
                return Ok(());
            }
            Err(TrySendError::Full(returned)) => {
                queued_chunks.fetch_sub(1, Ordering::Relaxed);
                queued_bytes.fetch_sub(bytes, Ordering::Relaxed);
                message = returned;
                thread::sleep(CHANNEL_WAIT);
            }
            Err(TrySendError::Disconnected(_)) => {
                queued_chunks.fetch_sub(1, Ordering::Relaxed);
                queued_bytes.fetch_sub(bytes, Ordering::Relaxed);
                return Ok(());
            }
        }
    }
}

fn update_peak_value(peak: &AtomicUsize, candidate: usize) {
    let mut prev = peak.load(Ordering::Relaxed);
    while candidate > prev {
        match peak.compare_exchange_weak(prev, candidate, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => prev = actual,
        }
    }
}

fn send_message(
    sender: &SyncSender<PartitionMessage>,
    mut message: PartitionMessage,
    stop: &AtomicBool,
    runtime: &ExecutionRuntime,
) -> Result<(), QueryError> {
    loop {
        if stop.load(Ordering::Relaxed) || runtime.is_cancelled() {
            return Ok(());
        }
        match sender.try_send(message) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                message = returned;
                thread::sleep(CHANNEL_WAIT);
            }
            Err(TrySendError::Disconnected(_)) => return Ok(()),
        }
    }
}

fn release_queue_metrics(queued_chunks: &AtomicUsize, queued_bytes: &AtomicUsize, bytes: usize) {
    queued_chunks.fetch_sub(1, Ordering::Relaxed);
    queued_bytes.fetch_sub(bytes, Ordering::Relaxed);
}
