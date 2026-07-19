//! MVCC Version Manager
//!
//! Provides timestamp management for MVCC (Multi-Version Concurrency Control)
//! based transaction isolation.
//!
//! ## Concurrency Model
//!
//! All write transactions are "insert" transactions that run concurrently.
//! Conflicts are detected by WriteSet at commit time, not at start time.
//! No write transaction ever blocks readers.
//!
//! This module uses `parking_lot::Condvar` for efficient waiting instead of
//! spin-wait loops. This reduces CPU usage during contention and provides
//! proper timeout support.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use super::snapshot_tracker::SnapshotTracker;
use crate::core::types::Timestamp;

/// Released timestamp sentinel value (0 means timestamp has been released)
/// Note: distinct from Timestamp::MAX which may be used as a sentinel elsewhere
pub const RELEASED_TIMESTAMP: Timestamp = 0;

#[derive(Debug, Clone, thiserror::Error)]
pub enum VersionManagerError {
    #[error("Too many concurrent transactions")]
    TooManyTransactions,

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(Timestamp),

    #[error("Timeout waiting for transaction")]
    Timeout,

    #[error("Failed to track snapshot for timestamp")]
    SnapshotTrackingFailed,
}

pub type VersionManagerResult<T> = Result<T, VersionManagerError>;

#[derive(Debug, Clone)]
pub struct VersionManagerConfig {
    pub max_concurrent_reads: u32,
    pub max_concurrent_inserts: u32,
    pub wait_timeout: Duration,
}

impl Default for VersionManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_reads: 1000,
            max_concurrent_inserts: 100,
            wait_timeout: Duration::from_secs(5),
        }
    }
}

impl VersionManagerConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_concurrent_reads(mut self, max: u32) -> Self {
        self.max_concurrent_reads = max;
        self
    }

    pub fn with_max_concurrent_inserts(mut self, max: u32) -> Self {
        self.max_concurrent_inserts = max;
        self
    }
}

pub struct VersionManager {
    write_ts: AtomicU32,
    read_ts: AtomicU32,
    pending_reqs: AtomicI32,
    lock: Mutex<()>,
    condvar: Condvar,
    config: VersionManagerConfig,
    snapshot_tracker: Arc<SnapshotTracker>,
}

impl VersionManager {
    pub fn new() -> Self {
        Self::with_config(VersionManagerConfig::default())
    }

    pub fn with_config(config: VersionManagerConfig) -> Self {
        Self {
            write_ts: AtomicU32::new(1),
            read_ts: AtomicU32::new(1),
            pending_reqs: AtomicI32::new(0),
            lock: Mutex::new(()),
            condvar: Condvar::new(),
            config,
            snapshot_tracker: Arc::new(SnapshotTracker::new()),
        }
    }

    pub fn init_ts(&self, ts: Timestamp) {
        self.write_ts.store(ts + 1, Ordering::SeqCst);
        self.read_ts.store(ts, Ordering::SeqCst);
    }

    pub fn clear(&self) {
        // Preserve write_ts so that subsequent writes and checkpoints
        // use timestamps >= the compact timestamp, ensuring persisted
        // data remains visible after reload.
        self.read_ts.store(0, Ordering::SeqCst);
        self.pending_reqs.store(0, Ordering::SeqCst);
    }

    pub fn write_timestamp(&self) -> Timestamp {
        self.write_ts.load(Ordering::SeqCst)
    }

    pub fn next_write_timestamp(&self) -> Timestamp {
        self.pending_reqs.fetch_add(1, Ordering::SeqCst);
        self.write_ts.fetch_add(1, Ordering::SeqCst)
    }

    pub fn read_timestamp(&self) -> Timestamp {
        self.read_ts.load(Ordering::SeqCst)
    }

    pub fn acquire_read_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_reads as i32 {
                    log::warn!(
                        "Too many pending read requests: {}. Max concurrent reads: {}. \
                        Consider increasing max_concurrent_reads or reducing read intensity.",
                        pr,
                        self.config.max_concurrent_reads,
                    );
                    self.condvar.wait(&mut guard);
                    continue;
                }
                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                let ts = self.read_ts.load(Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
                    self.condvar.notify_all();
                    return Err(VersionManagerError::SnapshotTrackingFailed);
                }
                return Ok(ts);
            }
            self.condvar.wait(&mut guard);
        }
    }

    pub fn acquire_read_timestamp_with_timeout(&self, timeout: Duration) -> Option<Timestamp> {
        let start = Instant::now();
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_reads as i32 {
                    log::warn!(
                        "Too many pending read requests: {}. Max concurrent reads: {}.",
                        pr,
                        self.config.max_concurrent_reads,
                    );
                    let elapsed = start.elapsed();
                    if elapsed >= timeout {
                        return None;
                    }
                    let remaining = timeout - elapsed;
                    let result = self.condvar.wait_for(&mut guard, remaining);
                    if result.timed_out() {
                        return None;
                    }
                    continue;
                }
                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                let ts = self.read_ts.load(Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
                    return None;
                }
                return Some(ts);
            }

            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return None;
            }

            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                return None;
            }
        }
    }

    pub fn release_read_timestamp(&self) {
        let ts = self.read_ts.load(Ordering::SeqCst);
        if let Err(e) = self.snapshot_tracker.release_snapshot(ts) {
            log::error!("Failed to release snapshot {}: {}", ts, e);
            // Continue anyway - we still need to decrement pending_reqs
        }
        self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    pub fn acquire_insert_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_inserts as i32 {
                    log::warn!(
                        "Too many pending insert requests: {}. Max concurrent inserts: {}. \
                        Consider increasing max_concurrent_inserts or reducing write intensity.",
                        pr,
                        self.config.max_concurrent_inserts,
                    );
                    self.condvar.wait(&mut guard);
                    continue;
                }

                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                let ts = self.write_ts.fetch_add(1, Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track insert snapshot {}: {}", ts, e);
                    self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
                    self.condvar.notify_all();
                    return Err(VersionManagerError::SnapshotTrackingFailed);
                }
                return Ok(ts);
            }
            self.condvar.wait(&mut guard);
        }
    }

    pub fn acquire_insert_timestamp_with_timeout(&self, timeout: Duration) -> Option<Timestamp> {
        let start = Instant::now();
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_inserts as i32 {
                    log::warn!(
                        "Too many pending insert requests: {}. Max concurrent inserts: {}.",
                        pr,
                        self.config.max_concurrent_inserts,
                    );
                    let elapsed = start.elapsed();
                    if elapsed >= timeout {
                        return None;
                    }
                    let remaining = timeout - elapsed;
                    let result = self.condvar.wait_for(&mut guard, remaining);
                    if result.timed_out() {
                        return None;
                    }
                    continue;
                }

                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                let ts = self.write_ts.fetch_add(1, Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track insert snapshot {}: {}", ts, e);
                    self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
                    return None;
                }
                return Some(ts);
            }

            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return None;
            }

            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                return None;
            }
        }
    }

    pub fn release_write_timestamp(&self, ts: Timestamp) {
        let _ = self.snapshot_tracker.release_snapshot(ts);
        let _guard = self.lock.lock();

        if ts > self.read_ts.load(Ordering::SeqCst) {
            self.read_ts.store(ts, Ordering::SeqCst);
        }

        self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
        drop(_guard);
        self.condvar.notify_all();
    }

    /// Release an insert timestamp.
    ///
    /// This name is kept at the transaction-manager boundary while the MVCC
    /// implementation uses the more accurate write-timestamp terminology.
    pub fn release_insert_timestamp(&self, ts: Timestamp) {
        self.release_write_timestamp(ts);
    }

    pub fn pending_count(&self) -> i32 {
        self.pending_reqs.load(Ordering::SeqCst)
    }

    pub fn get_safe_gc_timestamp(&self) -> Timestamp {
        self.snapshot_tracker.min_active_snapshot()
    }

    pub fn get_safe_gc_timestamp_with_margin(&self, margin: Timestamp) -> Timestamp {
        let safe_ts = self.snapshot_tracker.min_active_snapshot();
        safe_ts.saturating_sub(margin)
    }

    /// Get the snapshot tracker for explicit snapshot management
    pub fn snapshot_tracker(&self) -> &SnapshotTracker {
        &self.snapshot_tracker
    }
}

impl Default for VersionManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ReadTimestampGuard {
    version_manager: Arc<VersionManager>,
    timestamp: Timestamp,
}

impl ReadTimestampGuard {
    pub fn new(version_manager: Arc<VersionManager>) -> VersionManagerResult<Self> {
        let timestamp = version_manager.acquire_read_timestamp()?;
        Ok(Self {
            version_manager,
            timestamp,
        })
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}

impl Drop for ReadTimestampGuard {
    fn drop(&mut self) {
        self.version_manager.release_read_timestamp();
    }
}

pub struct InsertTimestampGuard {
    version_manager: Arc<VersionManager>,
    timestamp: Option<Timestamp>,
}

impl InsertTimestampGuard {
    pub fn new(version_manager: Arc<VersionManager>) -> VersionManagerResult<Self> {
        let timestamp = version_manager.acquire_insert_timestamp()?;
        Ok(Self {
            version_manager,
            timestamp: Some(timestamp),
        })
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp.unwrap_or(0)
    }

    pub fn commit(mut self) {
        if let Some(ts) = self.timestamp.take() {
            self.version_manager.release_insert_timestamp(ts);
        }
    }

    pub fn abort(mut self) {
        if let Some(ts) = self.timestamp.take() {
            self.version_manager.release_insert_timestamp(ts);
        }
    }
}

impl Drop for InsertTimestampGuard {
    fn drop(&mut self) {
        if let Some(ts) = self.timestamp.take() {
            self.version_manager.release_insert_timestamp(ts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_version_manager_basic() {
        let vm = VersionManager::new();

        let ts1 = vm.acquire_read_timestamp().expect("acquire read");
        assert_eq!(ts1, 1);
        vm.release_read_timestamp();

        let ts2 = vm.acquire_insert_timestamp().expect("acquire insert");
        assert!(ts2 >= 1);
        vm.release_insert_timestamp(ts2);
    }

    #[test]
    fn test_read_timestamp_guard() {
        let vm = Arc::new(VersionManager::new());

        {
            let guard = ReadTimestampGuard::new(vm.clone()).expect("guard should be created");
            assert_eq!(guard.timestamp(), 1);
        }

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_insert_timestamp_guard() {
        let vm = Arc::new(VersionManager::new());

        {
            let guard = InsertTimestampGuard::new(vm.clone()).expect("guard should be created");
            let ts = guard.timestamp();
            assert!(ts >= 1);
        }

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_concurrent_reads() {
        let vm = Arc::new(VersionManager::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let vm_clone = vm.clone();
            handles.push(thread::spawn(move || {
                let guard = ReadTimestampGuard::new(vm_clone).expect("guard should be created");
                thread::sleep(Duration::from_millis(10));
                guard.timestamp()
            }));
        }

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|&ts| ts == 1));
    }

    #[test]
    fn test_concurrent_inserts() {
        let vm = Arc::new(VersionManager::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let vm_clone = vm.clone();
            handles.push(thread::spawn(move || {
                let guard = InsertTimestampGuard::new(vm_clone).expect("guard should be created");
                let ts = guard.timestamp();
                thread::sleep(Duration::from_millis(10));
                ts
            }));
        }

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let unique: HashSet<_> = results.into_iter().collect();
        assert_eq!(unique.len(), 10);
    }

    #[test]
    fn test_snapshot_tracker_cleanup_threshold() {
        let vm = Arc::new(VersionManager::new());
        let tracker = vm.snapshot_tracker();

        // Add multiple snapshots via insert timestamps
        let ts1 = vm.acquire_insert_timestamp().expect("acquire insert");
        let ts2 = vm.acquire_insert_timestamp().expect("acquire insert");
        let ts3 = vm.acquire_insert_timestamp().expect("acquire insert");

        // Cleanup threshold should be minimum active
        assert_eq!(tracker.cleanup_threshold(), ts1);

        // Release first
        vm.release_insert_timestamp(ts1);
        assert_eq!(tracker.cleanup_threshold(), ts2);

        // Release second
        vm.release_insert_timestamp(ts2);
        assert_eq!(tracker.cleanup_threshold(), ts3);

        // Release last
        vm.release_insert_timestamp(ts3);
        assert_eq!(tracker.cleanup_threshold(), u32::MAX); // No active snapshots
    }
}
