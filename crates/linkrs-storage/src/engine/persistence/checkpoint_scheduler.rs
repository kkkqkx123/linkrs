use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use linkrs_core::StorageResult;
use linkrs_metrics::{CheckpointTriggerReason, StatsManager};

use crate::engine::persistence_coordinator::{
    CheckpointStats, PersistenceCoordinator, PersistenceStateGuard,
};
use crate::maintenance_scheduler::execute_guarded;
use crate::thread_pool::{BackgroundTaskHandle, StorageThreadPool};

/// Overrun reporting bound for one checkpoint execution. Warn-only: the
/// checkpoint is never cancelled, mirroring the maintenance-task timeout.
/// Default when no operation timeout is supplied; the scheduler prefers the
/// storage `operation_timeout` so tuning stays in one place.
#[allow(dead_code)]
const CHECKPOINT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Dedicated checkpoint execution threads. One poll driver plus one
/// execution lane is the fixed default; larger values are only for tests.
#[allow(dead_code)]
pub const CHECKPOINT_EXECUTOR_THREADS_DEFAULT: usize = 1;

#[derive(Debug, Clone, Copy)]
struct CheckpointRequest {
    requested_at: Instant,
    reason: CheckpointTriggerReason,
}

/// Background checkpoint scheduler.
///
/// Polling runs on a dedicated driver thread; executions run on a dedicated
/// single-thread channel decoupled from the shared compute pool. Table-level
/// fan-out inside an execution still uses the global parallel runtime, so the
/// execution lane blocks as an external caller without forming a pool cycle.
pub struct CheckpointScheduler {
    coordinator: Arc<parking_lot::RwLock<PersistenceCoordinator>>,
    thread_pool: Arc<StorageThreadPool>,
    execution_pool: Arc<StorageThreadPool>,
    execution_timeout: Duration,
    stats: Arc<Mutex<Option<Arc<StatsManager>>>>,
    pending: Arc<AtomicBool>,
    pending_request: Arc<Mutex<Option<CheckpointRequest>>>,
    /// In-flight checkpoint tasks spawned onto the pool. `stop` drains this
    /// count so shutdown cannot leave a checkpoint writing to the storage
    /// directory after the owning handle is gone.
    active: Arc<(Mutex<usize>, Condvar)>,
    handle: Option<BackgroundTaskHandle>,
    poll_interval: Duration,
    enabled: bool,
    executor: Arc<
        dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
            + Send
            + Sync,
    >,
}

impl CheckpointScheduler {
    #[allow(dead_code)]
    pub fn new(
        coordinator: Arc<parking_lot::RwLock<PersistenceCoordinator>>,
        thread_pool: Arc<StorageThreadPool>,
        stats: Option<Arc<StatsManager>>,
        poll_interval: Duration,
        enabled: bool,
        executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self::new_with_execution(
            coordinator,
            thread_pool,
            stats,
            poll_interval,
            enabled,
            executor,
            CHECKPOINT_EXECUTION_TIMEOUT,
            CHECKPOINT_EXECUTOR_THREADS_DEFAULT,
        )
    }

    pub fn new_with_execution(
        coordinator: Arc<parking_lot::RwLock<PersistenceCoordinator>>,
        thread_pool: Arc<StorageThreadPool>,
        stats: Option<Arc<StatsManager>>,
        poll_interval: Duration,
        enabled: bool,
        executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        >,
        execution_timeout: Duration,
        execution_threads: usize,
    ) -> Self {
        let execution_pool = Arc::new(
            StorageThreadPool::with_threads(execution_threads.max(1)).unwrap_or_default(),
        );
        Self {
            coordinator,
            thread_pool,
            execution_pool,
            execution_timeout,
            stats: Arc::new(Mutex::new(stats)),
            pending: Arc::new(AtomicBool::new(false)),
            pending_request: Arc::new(Mutex::new(None)),
            active: Arc::new((Mutex::new(0), Condvar::new())),
            handle: None,
            poll_interval,
            enabled,
            executor,
        }
    }

    /// Dedicated execution lanes (default one). The fixed background thread
    /// count is one driver plus these lanes, fully decoupled from the pool.
    pub fn execution_thread_count(&self) -> usize {
        self.execution_pool.num_threads()
    }

    /// Fixed background threads owned by the scheduler: driver plus channel.
    pub fn dedicated_thread_count(&self) -> usize {
        1 + self.execution_thread_count()
    }

    pub fn execution_timeout(&self) -> Duration {
        self.execution_timeout
    }

    pub fn set_stats(&self, stats: Option<Arc<StatsManager>>) {
        *self.stats.lock() = stats;
    }

    /// Request a checkpoint with the given reason.
    /// Non-blocking and deduplicated via AtomicBool.
    pub fn request_checkpoint(&self, reason: CheckpointTriggerReason) {
        if !self.enabled {
            return;
        }
        if self
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let mut guard = self.pending_request.lock();
            *guard = Some(CheckpointRequest {
                requested_at: Instant::now(),
                reason,
            });
        } else if let Some(stats) = self.stats.lock().as_ref() {
            stats.record_checkpoint_deduplicated();
        }
    }

    /// Start the periodic background polling task.
    pub fn start(&mut self) {
        if !self.enabled || self.handle.is_some() {
            return;
        }
        let coordinator = self.coordinator.clone();
        let stats = self.stats.clone();
        let pending = self.pending.clone();
        let pending_request = self.pending_request.clone();
        let executor = self.executor.clone();
        let active = self.active.clone();
        let execution_pool = self.execution_pool.clone();
        let execution_timeout = self.execution_timeout;

        let running = Arc::new(AtomicBool::new(false));
        let poll_interval = self.poll_interval;
        let handle = self.thread_pool.spawn_periodic(
            running,
            poll_interval,
            Duration::from_millis(10),
            move || {
                // Panic-safe poll pass: a failing pass is skipped and the
                // loop keeps running instead of dying silently. Timeout is
                // warn-only; the checkpoint itself runs on the dedicated
                // execution channel so the poll pass stays short.
                execute_guarded("checkpoint-poll", execution_timeout, || {
                // Passive polling: check if checkpoint is needed
                let should = {
                    let coord = coordinator.read();
                    coord.should_checkpoint()
                };
                if should {
                    let reason = {
                        let coord = coordinator.read();
                        coord
                            .checkpoint_trigger_reason()
                            .unwrap_or(CheckpointTriggerReason::TimeSinceLastCheckpoint)
                    };
                    if pending
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        let mut guard = pending_request.lock();
                        *guard = Some(CheckpointRequest {
                            requested_at: Instant::now(),
                            reason,
                        });
                    } else if let Some(s) = stats.lock().as_ref() {
                        s.record_checkpoint_deduplicated();
                    }
                }

                // Try to execute pending request
                let request_opt = {
                    let mut guard = pending_request.lock();
                    guard.take()
                };
                if let Some(req) = request_opt {
                    pending.store(false, Ordering::Release);
                    let age = req.requested_at.elapsed();
                    if let Some(s) = stats.lock().as_ref().cloned() {
                        s.record_checkpoint_trigger(req.reason, age);
                    }
                    match coordinator.read().try_enter_checkpoint() {
                        Ok(guard) => {
                            let stats_clone = stats.clone();
                            let exec = executor.clone();
                            let reason = req.reason;
                            let lane = execution_pool.clone();
                            let active_task = active.clone();
                            *active_task.0.lock() += 1;
                            // Dedicated execution lane decoupled from the shared
                            // pool: a slow checkpoint never occupies compute
                            // workers needed by foreground or GC tasks.
                            lane.spawn(move || {
                                let started = std::time::Instant::now();
                                let outcome = std::panic::catch_unwind(
                                    std::panic::AssertUnwindSafe(|| exec(guard, reason)),
                                );
                                if started.elapsed() > execution_timeout {
                                    log::warn!(
                                        "Checkpoint execution exceeded timeout: {:?} > {:?} (reason={:?})",
                                        started.elapsed(),
                                        execution_timeout,
                                        reason,
                                    );
                                }
                                match outcome {
                                    Ok(Ok(cs)) => {
                                        if let Some(s) = stats_clone.lock().as_ref().cloned() {
                                            s.record_checkpoint_success(
                                                cs.duration,
                                                cs.bytes_flushed,
                                                cs.wal_files_truncated as u64,
                                            );
                                        }
                                        log::info!(
                                            "Async checkpoint completed: id={} reason={:?}",
                                            cs.checkpoint_id,
                                            reason
                                        );
                                    }
                                    Ok(Err(e)) => {
                                        if let Some(s) = stats_clone.lock().as_ref().cloned() {
                                            s.record_checkpoint_failure();
                                        }
                                        log::warn!(
                                            "Async checkpoint failed (reason={:?}): {}",
                                            reason,
                                            e
                                        );
                                    }
                                    Err(_) => {
                                        if let Some(s) = stats_clone.lock().as_ref().cloned() {
                                            s.record_checkpoint_failure();
                                        }
                                        log::warn!(
                                            "Async checkpoint panicked (reason={:?}); counted as failure",
                                            reason,
                                        );
                                    }
                                }
                                // Always released, even after a panic: `stop`
                                // drains this count and must not hang on a
                                // dead checkpoint task.
                                let (lock, cv) = &*active_task;
                                let mut count = lock.lock();
                                *count = count.saturating_sub(1);
                                if *count == 0 {
                                    cv.notify_all();
                                }
                            });
                        }
                        Err(_) => {
                            if let Some(s) = stats.lock().as_ref().cloned() {
                                s.record_checkpoint_blocked();
                            }
                        }
                    }
                }
                    true
                });
            },
        );
        log::debug!(
            "Checkpoint scheduler started (dedicated_threads={} timeout={:?})",
            self.dedicated_thread_count(),
            self.execution_timeout(),
        );
        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.stop();
            handle.join();
        }
        self.pending.store(false, Ordering::Release);
        *self.pending_request.lock() = None;
        // Drain in-flight checkpoint tasks so a dropped handle cannot leave a
        // checkpoint writing to the directory a subsequent open recovers.
        let (lock, cv) = &*self.active;
        let mut count = lock.lock();
        while *count > 0 {
            cv.wait(&mut count);
        }
    }

    pub fn is_running(&self) -> bool {
        self.handle
            .as_ref()
            .map(|h| h.is_running())
            .unwrap_or(false)
    }

    pub fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    /// Number of checkpoint executions currently in flight on the dedicated
    /// lane. Reported in [`crate::maintenance_scheduler::MaintenanceOverview`]
    /// so a slow checkpoint is distinguishable from a stalled poll loop.
    pub fn active_count(&self) -> usize {
        *self.active.0.lock()
    }

    /// Monotonic checkpoint epoch hint for table commit manifests. Reads the
    /// published checkpoint sequence (the scheduler's single-flight order),
    /// so per-table commits share one global order without a new clock.
    pub fn epoch_hint(&self) -> u64 {
        self.coordinator
            .read()
            .manifest_manager
            .load_latest()
            .ok()
            .flatten()
            .map(|m| m.checkpoint_id.saturating_add(1))
            .unwrap_or(1)
            .max(1)
    }
}

impl Drop for CheckpointScheduler {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_deduplication() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = crate::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        let coordinator = Arc::new(parking_lot::RwLock::new(
            PersistenceCoordinator::new(config).unwrap(),
        ));
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let stats = Arc::new(StatsManager::new());
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        > = Arc::new(move |_g, _| {
            cc.fetch_add(1, Ordering::Relaxed);
            Err(linkrs_core::StorageError::db_error("injected"))
        });
        let scheduler = CheckpointScheduler::new(
            coordinator,
            pool,
            Some(stats.clone()),
            Duration::from_secs(1),
            true,
            executor,
        );
        // First request should be pending
        scheduler.request_checkpoint(CheckpointTriggerReason::WalSizeExceeded);
        assert!(scheduler.pending());
        // Second request should be deduplicated
        scheduler.request_checkpoint(CheckpointTriggerReason::Explicit);
        assert_eq!(
            stats.get_value(linkrs_metrics::MetricType::CheckpointRequestsDeduplicated),
            Some(1)
        );
        // Still pending with original reason
        assert!(scheduler.pending());
    }

    #[test]
    fn test_disabled_scheduler_does_not_accept_requests() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = crate::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        let coordinator = Arc::new(parking_lot::RwLock::new(
            PersistenceCoordinator::new(config).unwrap(),
        ));
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let stats = Arc::new(StatsManager::new());
        let executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        > = Arc::new(|_, _| Err(linkrs_core::StorageError::db_error("injected")));
        let scheduler = CheckpointScheduler::new(
            coordinator,
            pool,
            Some(stats),
            Duration::from_secs(1),
            false,
            executor,
        );
        scheduler.request_checkpoint(CheckpointTriggerReason::Explicit);
        assert!(!scheduler.pending());
    }

    #[test]
    fn test_blocked_checkpoint_records_metric() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = crate::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        let coordinator = Arc::new(parking_lot::RwLock::new(
            PersistenceCoordinator::new(config).unwrap(),
        ));
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let stats = Arc::new(StatsManager::new());
        // Hold the checkpoint lock externally so scheduler's try_enter fails.
        let _guard = coordinator.read().try_enter_checkpoint().unwrap();
        let executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        > = Arc::new(|_, _| {
            panic!("executor should not be called when checkpoint is already active");
        });
        let mut scheduler = CheckpointScheduler::new(
            coordinator.clone(),
            pool,
            Some(stats.clone()),
            Duration::from_millis(20),
            true,
            executor,
        );
        scheduler.request_checkpoint(CheckpointTriggerReason::Explicit);
        scheduler.start();
        std::thread::sleep(Duration::from_millis(100));
        scheduler.stop();
        assert_eq!(
            stats.get_value(linkrs_metrics::MetricType::CheckpointRequestsBlocked),
            Some(1)
        );
    }

    #[test]
    fn test_async_checkpoint_triggered_by_wal_size() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let mut config = crate::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        config.checkpoint_threshold = 10;
        config.max_wal_size = 100;
        config.auto_checkpoint_interval = Duration::from_secs(3600);
        config.async_checkpoint_poll_interval = Duration::from_millis(20);
        let coordinator = Arc::new(parking_lot::RwLock::new(
            PersistenceCoordinator::new(config).unwrap(),
        ));
        {
            let wal = coordinator.read().wal_manager().unwrap();
            wal.write()
                .append_entry(
                    linkrs_core::wal::types::WalOpType::InsertVertex,
                    0 as linkrs_core::types::Timestamp,
                    &[],
                )
                .unwrap();
        }
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let stats = Arc::new(StatsManager::new());
        let executed = Arc::new(AtomicUsize::new(0));
        let exec_cnt = executed.clone();
        let coord_for_exec = coordinator.clone();
        let executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        > = Arc::new(move |_guard, reason| {
            assert_eq!(reason, CheckpointTriggerReason::WalSizeExceeded);
            exec_cnt.fetch_add(1, Ordering::Relaxed);
            // Simulate a real checkpoint advancing the baseline so the next
            // poll sees should_checkpoint() == false.
            coord_for_exec
                .read()
                .mark_checkpointed(linkrs_transaction::wal::Lsn::new(40));
            Ok(CheckpointStats {
                checkpoint_id: 1,
                data_flushed: 100,
                wal_truncated: 10,
                duration: Duration::from_millis(5),
                snapshot_created: false,
                checkpoint_seq: 1,
                data_files_created: 1,
                bytes_flushed: 100,
                wal_files_truncated: 1,
                trigger_reason: reason,
                snapshot_timestamp: 0,
            })
        });
        let mut scheduler = CheckpointScheduler::new(
            coordinator.clone(),
            pool,
            Some(stats.clone()),
            Duration::from_millis(20),
            true,
            executor,
        );
        scheduler.start();
        // Wait for the periodic poll to detect should_checkpoint and execute
        for _ in 0..30 {
            if executed.load(Ordering::Relaxed) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Give the scheduler a brief grace period then shut down; at most one
        // successful checkpoint should have been recorded because the mock
        // executor advances last_checkpoint_lsn.
        std::thread::sleep(Duration::from_millis(40));
        scheduler.stop();
        assert!(
            executed.load(Ordering::Relaxed) >= 1,
            "checkpoint should have been triggered by WAL size"
        );
        assert_eq!(
            stats.get_value(linkrs_metrics::MetricType::CheckpointSuccessCount),
            Some(1)
        );
        assert_eq!(
            stats.get_value(linkrs_metrics::MetricType::CheckpointTriggeredByWalSize),
            Some(1)
        );
    }

    #[test]
    fn test_async_checkpoint_failure_does_not_block_commits() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = crate::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        let coordinator = Arc::new(parking_lot::RwLock::new(
            PersistenceCoordinator::new(config).unwrap(),
        ));
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let stats = Arc::new(StatsManager::new());
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        > = Arc::new(move |_g, _| {
            let c = cc.fetch_add(1, Ordering::Relaxed);
            if c == 0 {
                Err(linkrs_core::StorageError::db_error("injected failure"))
            } else {
                Ok(CheckpointStats {
                    checkpoint_id: 1,
                    data_flushed: 10,
                    wal_truncated: 10,
                    duration: Duration::from_millis(1),
                    snapshot_created: false,
                    checkpoint_seq: 1,
                    data_files_created: 1,
                    bytes_flushed: 10,
                    wal_files_truncated: 1,
                    trigger_reason: CheckpointTriggerReason::Explicit,
                    snapshot_timestamp: 0,
                })
            }
        });
        let mut scheduler = CheckpointScheduler::new(
            coordinator.clone(),
            pool,
            Some(stats.clone()),
            Duration::from_millis(20),
            true,
            executor,
        );
        scheduler.request_checkpoint(CheckpointTriggerReason::Explicit);
        scheduler.start();
        std::thread::sleep(Duration::from_millis(80));
        // First execution should have failed
        assert_eq!(
            stats.get_value(linkrs_metrics::MetricType::CheckpointFailureCount),
            Some(1)
        );
        // Request again after failure
        scheduler.request_checkpoint(CheckpointTriggerReason::Explicit);
        std::thread::sleep(Duration::from_millis(80));
        scheduler.stop();
        assert_eq!(
            stats.get_value(linkrs_metrics::MetricType::CheckpointSuccessCount),
            Some(1)
        );
        // Verify dedup didn't block second attempt
        assert_eq!(call_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn dedicated_channel_has_fixed_thread_count_and_timeout() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = crate::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        let coordinator = Arc::new(parking_lot::RwLock::new(
            PersistenceCoordinator::new(config).unwrap(),
        ));
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        > = Arc::new(|_, _| Err(linkrs_core::StorageError::db_error("injected")));
        let scheduler = CheckpointScheduler::new_with_execution(
            coordinator,
            pool,
            None,
            Duration::from_secs(1),
            true,
            executor,
            Duration::from_secs(30),
            1,
        );
        assert_eq!(scheduler.execution_thread_count(), 1);
        assert_eq!(scheduler.dedicated_thread_count(), 2);
        assert_eq!(scheduler.execution_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn slow_checkpoint_does_not_starve_pool_tasks() {
        use std::sync::mpsc;
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = crate::engine::PersistenceConfig::for_work_dir(temp_dir.path());
        let coordinator = Arc::new(parking_lot::RwLock::new(
            PersistenceCoordinator::new(config).unwrap(),
        ));
        let pool = Arc::new(StorageThreadPool::new().unwrap());
        let stats = Arc::new(StatsManager::new());
        let entered = Arc::new(AtomicUsize::new(0));
        let entered_clone = entered.clone();
        let release = Arc::new(parking_lot::Mutex::new(false));
        let release_clone = release.clone();
        let executor: Arc<
            dyn Fn(PersistenceStateGuard, CheckpointTriggerReason) -> StorageResult<CheckpointStats>
                + Send
                + Sync,
        > = Arc::new(move |_g, reason| {
            entered_clone.fetch_add(1, Ordering::SeqCst);
            for _ in 0..100 {
                if *release_clone.lock() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(CheckpointStats {
                checkpoint_id: 1,
                data_flushed: 0,
                wal_truncated: 0,
                duration: Duration::from_millis(1),
                snapshot_created: false,
                checkpoint_seq: 1,
                data_files_created: 0,
                bytes_flushed: 0,
                wal_files_truncated: 0,
                trigger_reason: reason,
                snapshot_timestamp: 0,
            })
        });
        let mut scheduler = CheckpointScheduler::new_with_execution(
            coordinator,
            pool.clone(),
            Some(stats),
            Duration::from_millis(10),
            true,
            executor,
            Duration::from_secs(30),
            1,
        );
        scheduler.request_checkpoint(CheckpointTriggerReason::Explicit);
        scheduler.start();
        for _ in 0..50 {
            if entered.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        assert_eq!(scheduler.active_count(), 1);
        // Pool tasks still progress while the dedicated lane is blocked.
        let (tx, rx) = mpsc::channel();
        pool.spawn(move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("pool task must progress during slow checkpoint");
        *release.lock() = true;
        scheduler.stop();
        assert_eq!(scheduler.active_count(), 0);
    }
}
