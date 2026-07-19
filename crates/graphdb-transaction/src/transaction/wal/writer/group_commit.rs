//! Group Commit Coordinator
//!
//! Coordinates fsync operations across multiple threads sharing the same WAL file.
//! Uses dual-sequence numbering (`appended_seq`, `durable_seq`) with `Condvar`
//! coordination so that one thread performs `fsync` on behalf of all waiters.

use std::fs::File;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use crate::core::wal::types::{WalError, WalResult};

/// Default timeout for group commit follower wait.
/// If the sync leader does not complete within this duration, followers return
/// a timeout error rather than blocking indefinitely.
pub const GROUP_COMMIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared coordinator for group commit.
///
/// Multiple threads append to the same WAL file, then call
/// [`GroupCommitCoordinator::append_and_wait`] to ensure durability.
/// The first thread to arrive becomes the "sync leader" and performs the
/// actual `fsync`; subsequent threads wait on a condvar and are woken when
/// the leader completes.
#[derive(Clone)]
pub struct GroupCommitCoordinator {
    inner: Arc<GroupCommitState>,
}

struct GroupCommitState {
    file: Mutex<File>,
    appended_seq: AtomicU64,
    durable_seq: AtomicU64,
    sync_in_progress: AtomicBool,
    commit_mutex: Mutex<()>,
    commit_condvar: Condvar,
}

impl GroupCommitCoordinator {
    fn sync_file(&self) -> WalResult<()> {
        let file = self.inner.file.lock();
        file.sync_all()
            .map_err(|e| WalError::IoError(e.to_string()))
    }

    pub fn new(file: File, start_lsn: u64) -> Self {
        Self {
            inner: Arc::new(GroupCommitState {
                file: Mutex::new(file),
                appended_seq: AtomicU64::new(start_lsn),
                durable_seq: AtomicU64::new(start_lsn),
                sync_in_progress: AtomicBool::new(false),
                commit_mutex: Mutex::new(()),
                commit_condvar: Condvar::new(),
            }),
        }
    }

    /// Update the file handle (e.g., after WAL rotation).
    pub fn update_file(&self, file: File) {
        *self.inner.file.lock() = file;
    }

    /// Record that data up to `appended_lsn` has been written to the file.
    pub fn record_appended(&self, appended_lsn: u64) {
        let mut current = self.inner.appended_seq.load(Ordering::Relaxed);
        while appended_lsn > current {
            match self.inner.appended_seq.compare_exchange_weak(
                current,
                appended_lsn,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Wait until the data up to `appended_lsn` is durable (fsynced to disk).
    ///
    /// Blocks until one of:
    /// - The requested LSN is covered by a completed fsync.
    /// - The `timeout` elapses (returns `WalError::GroupCommitTimeout`).
    ///
    /// If no sync is in progress, the calling thread becomes the sync leader
    /// and performs the fsync. Followers wait on a condvar.
    pub fn append_and_wait_timeout(
        &self,
        appended_lsn: u64,
        timeout: Duration,
    ) -> WalResult<()> {
        // Fast path: already durable
        if self.inner.durable_seq.load(Ordering::SeqCst) >= appended_lsn {
            return Ok(());
        }

        let deadline = std::time::Instant::now() + timeout;
        let mut guard = self.inner.commit_mutex.lock();

        loop {
            // Re-check after acquiring mutex
            if self.inner.durable_seq.load(Ordering::SeqCst) >= appended_lsn {
                return Ok(());
            }

            if !self.inner.sync_in_progress.load(Ordering::SeqCst) {
                // Become sync leader
                self.inner.sync_in_progress.store(true, Ordering::SeqCst);
                self.inner
                    .appended_seq
                    .fetch_max(appended_lsn, Ordering::SeqCst);

                // Release commit_mutex during fsync so other threads can join the queue
                drop(guard);

                let result = self.sync_file();

                let new_durable = self.inner.appended_seq.load(Ordering::SeqCst);
                self.inner.durable_seq.store(new_durable, Ordering::SeqCst);
                self.inner.sync_in_progress.store(false, Ordering::SeqCst);
                self.inner.commit_condvar.notify_all();

                return result;
            }

            // Wait as follower with timeout
            let now = std::time::Instant::now();
            if now >= deadline {
                self.inner.sync_in_progress.store(false, Ordering::SeqCst);
                self.inner.commit_condvar.notify_all();
                return Err(WalError::GroupCommitTimeout);
            }
            let remaining = deadline - now;
            let result = self.inner.commit_condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                self.inner.sync_in_progress.store(false, Ordering::SeqCst);
                self.inner.commit_condvar.notify_all();
                return Err(WalError::GroupCommitTimeout);
            }
        }
    }

    /// Wait until durable with default timeout (30s).
    ///
    /// Returns `WalError::GroupCommitTimeout` if the sync leader fails to complete
    /// within the timeout, preventing indefinite blocking on leader failure.
    pub fn append_and_wait(&self, appended_lsn: u64) -> WalResult<()> {
        self.append_and_wait_timeout(appended_lsn, GROUP_COMMIT_TIMEOUT)
    }

    /// Current durable sequence number.
    pub fn durable_seq(&self) -> u64 {
        self.inner.durable_seq.load(Ordering::SeqCst)
    }

    /// Current appended sequence number.
    pub fn appended_seq(&self) -> u64 {
        self.inner.appended_seq.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use tempfile::TempDir;

    use super::*;

    fn create_test_file() -> (TempDir, File) {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("test_wal");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .expect("create file");
        (dir, file)
    }

    #[test]
    fn test_single_thread_append_and_wait() {
        let (_dir, file) = create_test_file();
        let coordinator = GroupCommitCoordinator::new(file, 0);

        coordinator.record_appended(100);
        coordinator
            .append_and_wait(100)
            .expect("wait should succeed");
        assert!(coordinator.durable_seq() >= 100);
    }

    #[test]
    fn test_already_durable_returns_immediately() {
        let (_dir, file) = create_test_file();
        let coordinator = GroupCommitCoordinator::new(file, 0);

        coordinator.record_appended(100);
        coordinator.append_and_wait(100).expect("first wait");
        // Second call with same LSN should return immediately
        coordinator.append_and_wait(50).expect("already durable");
        assert!(coordinator.durable_seq() >= 100);
    }

    #[test]
    fn test_concurrent_append_and_wait() {
        let (_dir, file) = create_test_file();
        let coordinator = Arc::new(GroupCommitCoordinator::new(file, 0));

        let mut handles = Vec::new();
        for i in 0..4 {
            let coord = Arc::clone(&coordinator);
            handles.push(thread::spawn(move || {
                let lsn = (i + 1) * 100;
                coord.record_appended(lsn);
                coord.append_and_wait(lsn).expect("wait should succeed");
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic");
        }

        assert!(coordinator.durable_seq() >= 400);
    }

    #[test]
    fn test_update_file() {
        let (_dir, file) = create_test_file();
        let coordinator = GroupCommitCoordinator::new(file, 0);

        let (_dir2, file2) = create_test_file();
        coordinator.update_file(file2);

        coordinator.record_appended(50);
        coordinator
            .append_and_wait(50)
            .expect("wait after file update");
        assert!(coordinator.durable_seq() >= 50);
    }
}
