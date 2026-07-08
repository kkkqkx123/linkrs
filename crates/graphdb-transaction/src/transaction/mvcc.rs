//! MVCC Version Manager
//!
//! Provides timestamp management for MVCC (Multi-Version Concurrency Control)
//! based transaction isolation.
//!
//! ## Concurrency Model
//!
//! This module uses `parking_lot::Condvar` for efficient waiting instead of
//! spin-wait loops. This reduces CPU usage during contention and provides
//! proper timeout support.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex, RwLock};

use crate::core::types::Timestamp;
use super::snapshot_tracker::SnapshotTracker;

const RING_BUF_SIZE: u32 = 1024 * 1024;
const RING_INDEX_MASK: u32 = RING_BUF_SIZE - 1;

/// Safety check: log warning if pending operations approach ring buffer capacity.
/// This prevents silent corruption if concurrent transactions exceed buffer size.
const RING_BUF_WARNING_THRESHOLD: i32 = (RING_BUF_SIZE as i32) / 2;

#[derive(Debug, Clone, thiserror::Error)]
pub enum VersionManagerError {
    #[error("Too many concurrent transactions")]
    TooManyTransactions,

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(Timestamp),

    #[error("Update transaction already in progress")]
    UpdateInProgress,

    #[error("Timeout waiting for transaction")]
    Timeout,
}

pub type VersionManagerResult<T> = Result<T, VersionManagerError>;

#[derive(Debug, Default)]
struct BitSet {
    data: RwLock<Vec<u64>>,
}

impl BitSet {
    fn new(size: usize) -> Self {
        let word_count = size.div_ceil(64);
        Self {
            data: RwLock::new(vec![0u64; word_count]),
        }
    }

    fn set(&self, index: u32) {
        let word = index as usize / 64;
        let bit = index as usize % 64;
        let mut data = self.data.write();
        if word < data.len() {
            data[word] |= 1u64 << bit;
        }
    }

    fn atomic_reset_with_ret(&self, index: u32) -> bool {
        let word = index as usize / 64;
        let bit = index as usize % 64;
        let mut data = self.data.write();
        if word < data.len() {
            let mask = 1u64 << bit;
            let was_set = (data[word] & mask) != 0;
            if was_set {
                data[word] &= !mask;
                return true;
            }
        }
        false
    }

    fn reset_all(&self) {
        let mut data = self.data.write();
        for word in data.iter_mut() {
            *word = 0;
        }
    }
}

#[derive(Debug, Clone)]
pub struct VersionManagerConfig {
    pub max_concurrent_reads: u32,
    pub max_concurrent_inserts: u32,
    pub max_concurrent_updates: u32,
    pub thread_num: i32,
    pub wait_timeout: Duration,
    pub update_acquire_timeout: Duration,
    /// Enable partition-level conflict detection for updates (experimental)
    pub partition_conflict_detection: bool,
}

impl Default for VersionManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_reads: 1000,
            max_concurrent_inserts: 100,
            max_concurrent_updates: 1,
            thread_num: 1,
            wait_timeout: Duration::from_secs(5),
            update_acquire_timeout: Duration::from_secs(10),
            partition_conflict_detection: false,  // Disabled by default; enable for mixed workloads
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

    pub fn with_thread_num(mut self, num: i32) -> Self {
        self.thread_num = num;
        self
    }

    pub fn with_update_acquire_timeout(mut self, timeout: Duration) -> Self {
        self.update_acquire_timeout = timeout;
        self
    }

    pub fn with_partition_conflict_detection(mut self, enabled: bool) -> Self {
        self.partition_conflict_detection = enabled;
        self
    }
}

pub struct VersionManager {
    write_ts: AtomicU32,
    read_ts: AtomicU32,
    pending_reqs: AtomicI32,
    pending_update_reqs: AtomicI32,
    thread_num: AtomicI32,
    buffer: BitSet,
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
        let thread_num = config.thread_num;
        Self {
            write_ts: AtomicU32::new(1),
            read_ts: AtomicU32::new(1),
            pending_reqs: AtomicI32::new(0),
            pending_update_reqs: AtomicI32::new(0),
            thread_num: AtomicI32::new(thread_num),
            buffer: BitSet::new(RING_BUF_SIZE as usize),
            lock: Mutex::new(()),
            condvar: Condvar::new(),
            config,
            snapshot_tracker: Arc::new(SnapshotTracker::new()),
        }
    }

    pub fn init_ts(&self, ts: Timestamp, thread_num: i32) {
        self.write_ts.store(ts + 1, Ordering::SeqCst);
        self.read_ts.store(ts, Ordering::SeqCst);
        self.thread_num.store(thread_num, Ordering::SeqCst);
    }

    pub fn clear(&self) {
        // Preserve write_ts so that subsequent writes and checkpoints
        // use timestamps >= the compact timestamp, ensuring persisted
        // data remains visible after reload.
        self.read_ts.store(0, Ordering::SeqCst);
        self.pending_reqs.store(0, Ordering::SeqCst);
        self.pending_update_reqs.store(0, Ordering::SeqCst);
        self.buffer.reset_all();
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

    pub fn acquire_read_timestamp(&self) -> Timestamp {
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                // Safety check: ensure pending_reqs won't overflow RING_BUF_SIZE
                // This prevents timestamp collision in the ring buffer indexing
                if pr >= (RING_BUF_SIZE as i32 - 1) {
                    log::warn!(
                        "Too many pending read requests: {}. Ring buffer capacity: {}. \
                        Consider increasing max_concurrent_reads or reducing read intensity.",
                        pr, RING_BUF_SIZE
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
                    panic!("Critical: Failed to track snapshot for timestamp {}", ts);
                }
                return ts;
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
                // Safety check: ensure pending_reqs won't overflow RING_BUF_SIZE
                if pr >= (RING_BUF_SIZE as i32 - 1) {
                    log::warn!(
                        "Too many pending read requests: {}. Ring buffer capacity: {}.",
                        pr, RING_BUF_SIZE
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

    pub fn acquire_insert_timestamp(&self) -> Timestamp {
        let mut guard = self.lock.lock();
        loop {
            let pr = self.pending_reqs.load(Ordering::SeqCst);
            if pr >= 0 {
                // Safety check: ensure pending_reqs won't overflow RING_BUF_SIZE
                // This prevents timestamp collision in the ring buffer indexing
                if pr >= (RING_BUF_SIZE as i32 - 1) {
                    log::warn!(
                        "Too many pending insert requests: {}. Ring buffer capacity: {}. \
                        Consider increasing max_concurrent_inserts or reducing write intensity.",
                        pr, RING_BUF_SIZE
                    );
                    self.condvar.wait(&mut guard);
                    continue;
                }

                // Warning threshold for monitoring
                if pr >= RING_BUF_WARNING_THRESHOLD {
                    log::warn!(
                        "Ring buffer approaching saturation: {} concurrent transactions (capacity: {})",
                        pr, RING_BUF_SIZE
                    );
                }

                self.pending_reqs.fetch_add(1, Ordering::SeqCst);
                let ts = self.write_ts.fetch_add(1, Ordering::SeqCst);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track insert snapshot {}: {}", ts, e);
                    self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
                    self.condvar.notify_all();
                    panic!("Critical: Failed to track snapshot for timestamp {}", ts);
                }
                return ts;
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
                // Safety check: ensure pending_reqs won't overflow RING_BUF_SIZE
                if pr >= (RING_BUF_SIZE as i32 - 1) {
                    log::warn!(
                        "Too many pending insert requests: {}. Ring buffer capacity: {}.",
                        pr, RING_BUF_SIZE
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

                // Warning threshold for monitoring
                if pr >= RING_BUF_WARNING_THRESHOLD {
                    log::warn!(
                        "Ring buffer approaching saturation: {} concurrent transactions (capacity: {})",
                        pr, RING_BUF_SIZE
                    );
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

    pub fn release_insert_timestamp(&self, ts: Timestamp) {
        let _ = self.snapshot_tracker.release_snapshot(ts);
        let _guard = self.lock.lock();

        let current_read_ts = self.read_ts.load(Ordering::SeqCst);
        if ts >= current_read_ts {
            while self
                .buffer
                .atomic_reset_with_ret((ts + 1) & RING_INDEX_MASK)
            {}
            self.read_ts.store(ts, Ordering::SeqCst);
        } else {
            self.buffer.set(ts & RING_INDEX_MASK);
        }

        self.pending_reqs.fetch_sub(1, Ordering::SeqCst);
        drop(_guard);
        self.condvar.notify_all();
    }

    pub fn acquire_update_timestamp(&self) -> VersionManagerResult<Timestamp> {
        self.acquire_update_timestamp_with_timeout(self.config.update_acquire_timeout)
    }

    /// Acquire an exclusive update timestamp for a transaction.
    ///
    /// Current implementation (SERIALIZABLE isolation):
    /// - Only 1 concurrent update transaction allowed (max_concurrent_updates=1 by default)
    /// - Waits for all active read/insert transactions to complete
    /// - Then waits for all pending operations to finish before proceeding
    ///
    /// This ensures perfect isolation but limits concurrency. For mixed workloads with
    /// many reads and few updates, consider enabling partition_conflict_detection to
    /// allow concurrent updates that don't conflict (experimental feature).
    ///
    /// # Performance Notes
    /// - Single update: O(1) timestamp allocation, O(N) wait for N active reads
    /// - Concurrent updates: Not allowed (will block)
    /// - Read-heavy workloads: Updates are blocked; consider horizontal sharding
    ///
    /// Future optimization: Implement row/partition-level conflict detection to
    /// allow non-conflicting updates to proceed concurrently.
    pub fn acquire_update_timestamp_with_timeout(
        &self,
        timeout: Duration,
    ) -> VersionManagerResult<Timestamp> {
        let start = Instant::now();
        let mut guard = self.lock.lock();

        while self
            .pending_update_reqs
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(VersionManagerError::Timeout);
            }

            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                return Err(VersionManagerError::Timeout);
            }
        }

        let thread_num = self.thread_num.load(Ordering::SeqCst);
        self.pending_reqs.fetch_sub(thread_num, Ordering::SeqCst);

        let target = -thread_num;
        while self.pending_reqs.load(Ordering::SeqCst) != target {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                self.pending_reqs.fetch_add(thread_num, Ordering::SeqCst);
                self.pending_update_reqs.store(0, Ordering::SeqCst);
                return Err(VersionManagerError::Timeout);
            }

            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                self.pending_reqs.fetch_add(thread_num, Ordering::SeqCst);
                self.pending_update_reqs.store(0, Ordering::SeqCst);
                return Err(VersionManagerError::Timeout);
            }
        }

        let ts = self.write_ts.fetch_add(1, Ordering::SeqCst);
        drop(guard);
        let _ = self.snapshot_tracker.add_snapshot(ts);
        Ok(ts)
    }

    pub fn release_update_timestamp(&self, ts: Timestamp) {
        let _ = self.snapshot_tracker.release_snapshot(ts);
        let _guard = self.lock.lock();

        if ts == self.read_ts.load(Ordering::SeqCst) + 1 {
            self.read_ts.store(ts, Ordering::SeqCst);
        } else {
            self.buffer.set(ts & RING_INDEX_MASK);
        }

        self.pending_reqs
            .fetch_add(self.thread_num.load(Ordering::SeqCst), Ordering::SeqCst);
        self.pending_update_reqs.store(0, Ordering::SeqCst);
        drop(_guard);
        self.condvar.notify_all();
    }

    pub fn revert_update_timestamp(&self, ts: Timestamp) -> bool {
        let expected = ts + 1;
        if self
            .write_ts
            .compare_exchange(expected, ts, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = self.snapshot_tracker.release_snapshot(expected);
            self.pending_reqs
                .fetch_add(self.thread_num.load(Ordering::SeqCst), Ordering::SeqCst);
            self.pending_update_reqs.store(0, Ordering::SeqCst);
            self.condvar.notify_all();
            return true;
        }
        false
    }

    pub fn pending_count(&self) -> i32 {
        self.pending_reqs.load(Ordering::SeqCst)
    }

    pub fn is_update_in_progress(&self) -> bool {
        self.pending_update_reqs.load(Ordering::SeqCst) > 0
    }

    pub fn get_safe_gc_timestamp(&self) -> Timestamp {
        self.read_ts.load(Ordering::SeqCst)
    }

    pub fn get_safe_gc_timestamp_with_margin(&self, margin: Timestamp) -> Timestamp {
        let read_ts = self.read_ts.load(Ordering::SeqCst);
        read_ts.saturating_sub(margin)
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
    pub fn new(version_manager: Arc<VersionManager>) -> Self {
        let timestamp = version_manager.acquire_read_timestamp();
        Self {
            version_manager,
            timestamp,
        }
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
    pub fn new(version_manager: Arc<VersionManager>) -> Self {
        let timestamp = version_manager.acquire_insert_timestamp();
        Self {
            version_manager,
            timestamp: Some(timestamp),
        }
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

pub struct UpdateTimestampGuard {
    version_manager: Arc<VersionManager>,
    timestamp: Option<Timestamp>,
}

impl UpdateTimestampGuard {
    pub fn new(version_manager: Arc<VersionManager>) -> VersionManagerResult<Self> {
        let timestamp = version_manager.acquire_update_timestamp()?;
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
            self.version_manager.release_update_timestamp(ts);
        }
    }

    pub fn abort(mut self) {
        if let Some(ts) = self.timestamp.take() {
            self.version_manager.revert_update_timestamp(ts);
        }
    }
}

impl Drop for UpdateTimestampGuard {
    fn drop(&mut self) {
        if let Some(ts) = self.timestamp.take() {
            self.version_manager.release_update_timestamp(ts);
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

        let ts1 = vm.acquire_read_timestamp();
        assert_eq!(ts1, 1);
        vm.release_read_timestamp();

        let ts2 = vm.acquire_insert_timestamp();
        assert!(ts2 >= 1);
        vm.release_insert_timestamp(ts2);
    }

    #[test]
    fn test_read_timestamp_guard() {
        let vm = Arc::new(VersionManager::new());

        {
            let guard = ReadTimestampGuard::new(vm.clone());
            assert_eq!(guard.timestamp(), 1);
        }

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_insert_timestamp_guard() {
        let vm = Arc::new(VersionManager::new());

        {
            let guard = InsertTimestampGuard::new(vm.clone());
            let ts = guard.timestamp();
            assert!(ts >= 1);
        }

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_update_timestamp_guard() {
        let vm = Arc::new(VersionManager::new());

        {
            let guard = UpdateTimestampGuard::new(vm.clone()).expect("Failed to acquire update");
            let ts = guard.timestamp();
            assert!(ts >= 1);
        }

        assert!(!vm.is_update_in_progress());
    }

    #[test]
    fn test_concurrent_reads() {
        let vm = Arc::new(VersionManager::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let vm_clone = vm.clone();
            handles.push(thread::spawn(move || {
                let guard = ReadTimestampGuard::new(vm_clone);
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
                let guard = InsertTimestampGuard::new(vm_clone);
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
        let ts1 = vm.acquire_insert_timestamp();
        let ts2 = vm.acquire_insert_timestamp();
        let ts3 = vm.acquire_insert_timestamp();

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
        assert_eq!(tracker.cleanup_threshold(), u32::MAX);  // No active snapshots
    }
}
