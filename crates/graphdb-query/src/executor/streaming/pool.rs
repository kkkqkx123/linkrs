//! Morsel-style query-level bounded worker pool for dynamic partition execution.
//!
//! Workers are created once per query. Partition tasks are dynamically claimed
//! from a shared atomic counter — a "morsel queue" — providing natural load
//! balancing: faster workers automatically process more partitions.
//!
//! This is the only parallel execution mechanism (replaces the per-Gather
//! `ParallelPartitionCoordinator`). The [`TaskScheduler`] trait abstracts
//! over pool implementations.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::core::error::QueryError;
use crate::executor::base::{MemoryBudget, MemoryReservation};

use super::chunk::DataChunk;
use super::executor::StreamingExecutor;
use super::runtime::ExecutionRuntime;

const CHANNEL_WAIT: Duration = Duration::from_millis(2);

/// Abstract task scheduler for parallel partition execution.
///
/// Implementations manage a set of worker threads that dynamically claim
/// partition tasks from a shared atomic counter for natural load balancing.
pub trait TaskScheduler: Send + Sync + std::fmt::Debug {
    /// Submit a batch of partition tasks for execution.
    fn submit(&self, batch: Arc<PartitionBatch>);
    /// Number of worker threads in this scheduler.
    fn max_workers(&self) -> usize;
}

/// Engine-level shared scheduler (M6).
///
/// Created once at database startup and reused across all queries.  Each
/// query receives an `Arc` reference through its bindings.  Workers are
/// created once and persist across query boundaries, eliminating the
/// per-query thread overhead of the old `MorselWorkerPool` pattern.
///
/// Query teardown does not join or shut down the shared workers — only
/// engine shutdown calls [`SharedScheduler::shutdown`].
#[derive(Debug)]
pub struct SharedScheduler {
    pool: Arc<MorselWorkerPool>,
}

impl SharedScheduler {
    /// Create a new shared scheduler with `max_workers` persistent threads.
    ///
    /// At least one worker is always created.  Pass `0` to use the default
    /// (typically number of CPU cores, minimum 1).
    pub fn new(max_workers: usize) -> Self {
        Self {
            pool: Arc::new(MorselWorkerPool::new(max_workers.max(1))),
        }
    }

    /// Inject this scheduler into the given runtime.
    ///
    /// This makes the runtime's worker pool point to the shared scheduler's
    /// underlying [`MorselWorkerPool`], so that all Exchange/Gather operators
    /// in that query use the shared worker threads.
    pub fn apply_to_runtime(&self, runtime: &super::runtime::ExecutionRuntime) {
        runtime.set_shared_scheduler_raw(Some(self.pool.clone() as Arc<dyn TaskScheduler>));
    }

    /// Number of workers in this scheduler.
    pub fn max_workers(&self) -> usize {
        self.pool.max_workers()
    }

    /// Shut down all workers and wait for them to exit.
    ///
    /// Called during engine shutdown.  After this, no further batches can
    /// be submitted.  Must only be called once no more queries need the
    /// scheduler.
    pub fn shutdown(&mut self) {
        // Arc::get_mut only succeeds when no other references exist.
        // During orderly shutdown this should be the case because all
        // queries have completed.
        if let Some(pool) = Arc::get_mut(&mut self.pool) {
            pool.shutdown();
        }
        // If other references remain, workers will exit when their
        // stop flag or channel closure is detected (via detach on drop).
    }

    /// Shut down workers through a shared reference.
    ///
    /// Signals the stop flag and closes the batch channel so workers exit
    /// promptly, even while other references (in-flight query runtimes)
    /// still exist.  Worker threads are not joined here; they terminate
    /// after observing the stop flag / closed channel.
    pub fn shutdown_shared(&self) {
        self.pool.shutdown_shared();
    }
}

#[derive(Debug)]
pub struct BufferedChunk {
    pub(crate) chunk: DataChunk,
    pub(crate) _reservation: MemoryReservation,
    pub(crate) bytes: usize,
}

#[derive(Debug)]
pub enum PartitionMessage {
    Chunk(BufferedChunk),
    Finished,
}

/// Run a partition tree, sending output chunks through the given sender.
/// This is the core worker loop: open, advance in a loop, close.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_partition(
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
        let pre_count = queued_chunks.fetch_add(1, Ordering::Relaxed);
        let pre_bytes = queued_bytes.fetch_add(bytes, Ordering::Relaxed);
        match sender.try_send(message) {
            Ok(()) => {
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

pub(crate) fn update_peak_value(peak: &AtomicUsize, candidate: usize) {
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

pub(crate) fn release_queue_metrics(
    queued_chunks: &AtomicUsize,
    queued_bytes: &AtomicUsize,
    bytes: usize,
) {
    queued_chunks.fetch_sub(1, Ordering::Relaxed);
    queued_bytes.fetch_sub(bytes, Ordering::Relaxed);
}

/// A batch of partition tasks submitted to the pool for execution.
///
/// Workers dynamically claim partition indices via `next_index` and execute
/// the corresponding tree, sending output chunks through `senders[partition_id]`.
/// Partition trees are wrapped in `Mutex` to satisfy `Send + Sync` for `Arc<PartitionBatch>`
/// (each index is claimed by at most one worker via the atomic counter, so contention
/// on the mutex is limited to the brief `take()` window).
pub struct PartitionBatch {
    pub partitions: Vec<Mutex<Option<StreamingExecutor>>>,
    pub senders: Vec<SyncSender<PartitionMessage>>,
    pub error_tx: Sender<(usize, QueryError)>,
    pub next_index: AtomicUsize,
    pub total: usize,
    pub runtime: Arc<ExecutionRuntime>,
    pub worker_time_us: Arc<AtomicU64>,
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
    ) -> (
        Self,
        Vec<Receiver<PartitionMessage>>,
        Receiver<(usize, QueryError)>,
    ) {
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
            partitions: partitions
                .into_iter()
                .map(|p| Mutex::new(Some(p)))
                .collect(),
            senders,
            error_tx,
            next_index: AtomicUsize::new(0),
            total: partition_count,
            runtime,
            worker_time_us: Arc::new(AtomicU64::new(0)),
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
/// access via `next_for_partition()`.
#[derive(Debug)]
pub struct PartitionHandle {
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
    worker_count: usize,
}

impl PartitionHandle {
    /// Create a handle from a batch and its associated receivers.
    pub(crate) fn from_batch(
        batch: &Arc<PartitionBatch>,
        receivers: Vec<Receiver<PartitionMessage>>,
        error_rx: Receiver<(usize, QueryError)>,
        runtime: Arc<ExecutionRuntime>,
        started_at: Instant,
        worker_count: usize,
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
            worker_time_us: Arc::clone(&batch.worker_time_us),
            started_at,
            profile_recorded: false,
            runtime,
            worker_count,
        }
    }

    /// Pull one chunk from an individual partition, blocking until data is
    /// available instead of polling at the channel wait interval.
    ///
    /// This is the drain fast path for the [`GatherOperator::Concatenate`]
    /// gather, which reads exactly one partition at a time: a blocking
    /// receive wakes the instant the producer sends a chunk, removing the
    /// up-to-`CHANNEL_WAIT` poll latency from the steady-state drain and
    /// avoiding producer/consumer wakeup gaps.  Cancellation and worker
    /// errors are still observed at each chunk boundary because producers
    /// exit promptly on `stop`/cancel, which disconnects their sender.
    pub fn next_for_partition_blocking(
        &mut self,
        partition_id: usize,
    ) -> Result<Option<DataChunk>, QueryError> {
        if partition_id >= self.receivers.len() {
            return Err(QueryError::execution(format!(
                "Morsel partition handle has no partition {partition_id}",
            )));
        }
        match self.receivers[partition_id].recv() {
            Ok(PartitionMessage::Chunk(buffered)) => {
                release_queue_metrics(&self.buffered_chunks, &self.buffered_bytes, buffered.bytes);
                Ok(Some(buffered.chunk))
            }
            Ok(PartitionMessage::Finished) => Ok(None),
            Err(_) => {
                self.check_worker_error()?;
                if self.runtime.is_cancelled() {
                    let _ = self.stop_and_join();
                    return Err(QueryError::execution("Query cancelled".to_string()));
                }
                let _ = self.stop_and_join();
                Err(QueryError::execution(format!(
                    "Morsel partition {partition_id} disconnected before completion",
                )))
            }
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
        let board = self.runtime.profile();
        board.parallel_wall_time_us.fetch_add(
            self.started_at.elapsed().as_micros() as u64,
            Ordering::Relaxed,
        );
        board.parallel_work_time_us.fetch_add(
            self.worker_time_us.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        // Report the number of workers that actually executed partitions:
        // at most one worker per partition, so it is bounded by the
        // partition count even when the pool has more threads.
        board.parallel_workers.fetch_max(
            self.worker_count.min(self.partition_count),
            Ordering::Relaxed,
        );
        board.parallel_buffered_chunks_peak.fetch_max(
            self.buffered_chunks_peak.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        board.parallel_buffered_bytes_peak.fetch_max(
            self.buffered_bytes_peak.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
}

/// Query-level bounded worker pool with morsel-style dynamic task assignment.
///
/// Workers are created once per query and persist across multiple Exchange
/// node invocations. Partition tasks are dynamically claimed from a shared
/// atomic counter, so faster workers naturally process more work — this
/// eliminates the static round-robin load imbalance of per-Gather threads.
pub struct MorselWorkerPool {
    batch_tx: Mutex<Option<Sender<Arc<PartitionBatch>>>>,
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
                    Ok(batch) => {
                        // Clone channels/stop before moving batch into catch_unwind
                        // so we can propagate the panic as a query error.
                        let error_tx = batch.error_tx.clone();
                        let stop = batch.stop.clone();

                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            Self::process_batch(batch, &stopper)
                        }));
                        if let Err(panic_payload) = result {
                            let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                                s.to_string()
                            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "Worker panicked".to_string()
                            };
                            log::error!("Morsel worker panicked: {msg}");
                            stop.store(true, Ordering::Relaxed);
                            let _ = error_tx.send((usize::MAX, QueryError::execution(msg)));
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }));
        }

        Self {
            batch_tx: Mutex::new(Some(batch_tx)),
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
        let guard = self.batch_tx.lock().unwrap();
        if let Some(ref tx) = *guard {
            let workers_to_notify = self.max_workers.min(batch.total);
            for _ in 0..workers_to_notify {
                if tx.send(batch.clone()).is_err() {
                    break;
                }
            }
        }
    }

    /// Number of workers in this pool.
    pub fn max_workers(&self) -> usize {
        self.max_workers
    }

    /// Signal all workers to stop and wait for them to exit.
    ///
    /// Must be called from a non-worker thread — never from within
    /// a worker that holds the final runtime reference.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        *self.batch_tx.get_mut().unwrap() = None;
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }

    /// Signal all workers to stop through a shared reference.
    ///
    /// Sets the stop flag and closes the batch channel.  Workers blocked on
    /// `recv` observe the closed channel and exit; the stop flag covers
    /// workers between batches.  Safe to call while queries still hold
    /// runtime references to this pool.
    pub fn shutdown_shared(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = self.batch_tx.lock() {
            *guard = None;
        }
    }

    /// Signal workers to stop without joining.
    ///
    /// Safe to call from any context (including a worker that may own
    /// the last `Arc<ExecutionRuntime>`). Workers exit naturally when
    /// they observe the stop flag or the closed channel.
    pub fn detach(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        *self.batch_tx.get_mut().unwrap() = None;
        self.workers.clear();
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
        // Use detach (not shutdown) to avoid joining our own thread when
        // the last runtime reference is dropped inside a worker context.
        self.detach();
    }
}

impl TaskScheduler for MorselWorkerPool {
    fn submit(&self, batch: Arc<PartitionBatch>) {
        self.submit(batch);
    }

    fn max_workers(&self) -> usize {
        self.max_workers
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
