use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use graphdb_core::stats::{CheckpointTriggerReason, StatsManager};
use graphdb_core::StorageResult;

use crate::engine::persistence_coordinator::{
    CheckpointStats, PersistenceCoordinator, PersistenceStateGuard,
};
use crate::thread_pool::{BackgroundTaskHandle, StorageThreadPool};

#[derive(Debug, Clone, Copy)]
struct CheckpointRequest {
    requested_at: Instant,
    reason: CheckpointTriggerReason,
}

/// Background checkpoint scheduler.
///
/// Monitors WAL size and time since last checkpoint, triggering
/// non-blocking checkpoint operations on the shared thread pool.
pub struct CheckpointScheduler {
    coordinator: Arc<parking_lot::RwLock<PersistenceCoordinator>>,
    thread_pool: Arc<StorageThreadPool>,
    stats: Arc<Mutex<Option<Arc<StatsManager>>>>,
    pending: Arc<AtomicBool>,
    pending_request: Arc<Mutex<Option<CheckpointRequest>>>,
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
        Self {
            coordinator,
            thread_pool,
            stats: Arc::new(Mutex::new(stats)),
            pending: Arc::new(AtomicBool::new(false)),
            pending_request: Arc::new(Mutex::new(None)),
            handle: None,
            poll_interval,
            enabled,
            executor,
        }
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
        let thread_pool_clone = self.thread_pool.clone();

        let running = Arc::new(AtomicBool::new(false));
        let poll_interval = self.poll_interval;
        let handle = self.thread_pool.spawn_periodic(
            running,
            poll_interval,
            Duration::from_millis(10),
            move || {
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
                            let pool = thread_pool_clone.clone();
                            // Spawn checkpoint on the shared pool so the periodic
                            // poll loop is never blocked by a long-running checkpoint.
                            pool.spawn(move || match exec(guard, reason) {
                                Ok(cs) => {
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
                                Err(e) => {
                                    if let Some(s) = stats_clone.lock().as_ref().cloned() {
                                        s.record_checkpoint_failure();
                                    }
                                    log::warn!(
                                        "Async checkpoint failed (reason={:?}): {}",
                                        reason,
                                        e
                                    );
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
            },
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
            Err(graphdb_core::StorageError::db_error("injected"))
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
            stats.get_value(graphdb_core::stats::MetricType::CheckpointRequestsDeduplicated),
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
        > = Arc::new(|_, _| Err(graphdb_core::StorageError::db_error("injected")));
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
            stats.get_value(graphdb_core::stats::MetricType::CheckpointRequestsBlocked),
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
                .set_current_lsn(graphdb_transaction::wal::Lsn::new(20))
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
                .mark_checkpointed(graphdb_transaction::wal::Lsn::new(20));
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
        assert_eq!(executed.load(Ordering::Relaxed), 1);
        assert_eq!(
            stats.get_value(graphdb_core::stats::MetricType::CheckpointSuccessCount),
            Some(1)
        );
        assert_eq!(
            stats.get_value(graphdb_core::stats::MetricType::CheckpointTriggeredByWalSize),
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
                Err(graphdb_core::StorageError::db_error("injected failure"))
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
            stats.get_value(graphdb_core::stats::MetricType::CheckpointFailureCount),
            Some(1)
        );
        // Request again after failure
        scheduler.request_checkpoint(CheckpointTriggerReason::Explicit);
        std::thread::sleep(Duration::from_millis(80));
        scheduler.stop();
        assert_eq!(
            stats.get_value(graphdb_core::stats::MetricType::CheckpointSuccessCount),
            Some(1)
        );
        // Verify dedup didn't block second attempt
        assert_eq!(call_count.load(Ordering::Relaxed), 2);
    }
}
