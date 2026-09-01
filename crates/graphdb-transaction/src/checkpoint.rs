//! Checkpoint coordination
//!
//! Pauses new write transactions and waits for active writes to complete so
//! a checkpoint can capture a consistent snapshot. Modeled after Ladybug's
//! checkpoint isolation where checkpoint blocks new writes and drains active
//! ones.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use super::error::TransactionError;
use super::manager::TransactionManager;
use super::types::TransactionId;
use graphdb_core::types::Timestamp;

/// Checkpoint coordination gate.
///
/// pauses new write transactions and waits for active writes to complete,
/// then allows checkpoint to proceed. Modeled after Ladybug's checkpoint
/// isolation where checkpoint blocks new writes and drains active ones.
///
/// Phase 1 (two-phase checkpoint) extends this with early gate release:
/// after the WAL has been rotated (the only operation that requires write
/// exclusion), writes are resumed while the checkpoint's storage phase
/// (shadow pages, catalog serialization) continues in the background.
pub struct CheckpointGate {
    /// When true, no new write transactions are allowed.
    writing_paused: AtomicBool,
    /// Whether a checkpoint is active (even after early release). Used to
    /// expose checkpoint liveness without blocking writes.
    checkpoint_active: AtomicBool,
    /// Whether checkpoint is in storage phase (writes already released).
    in_storage_phase: AtomicBool,
    /// Number of active write transactions currently in-flight.
    /// Used to determine when the gate has fully drained.
    active_writes: AtomicU64,
    /// Condvar for checkpoint thread to wait on until active_writes reaches 0.
    condvar: Condvar,
    /// Mutex protecting the condvar.
    mutex: Mutex<()>,
}

impl CheckpointGate {
    pub fn new() -> Self {
        Self {
            writing_paused: AtomicBool::new(false),
            checkpoint_active: AtomicBool::new(false),
            in_storage_phase: AtomicBool::new(false),
            active_writes: AtomicU64::new(0),
            condvar: Condvar::new(),
            mutex: Mutex::new(()),
        }
    }

    /// Attempt to acquire a write slot. Returns Err if writes are paused.
    pub fn acquire_write(&self) -> Result<(), TransactionError> {
        if self.writing_paused.load(Ordering::SeqCst) {
            return Err(TransactionError::checkpoint_in_progress());
        }
        self.active_writes.fetch_add(1, Ordering::SeqCst);
        // Re-check after incrementing: if paused between the two atomics, bail out.
        if self.writing_paused.load(Ordering::SeqCst) {
            self.active_writes.fetch_sub(1, Ordering::SeqCst);
            return Err(TransactionError::checkpoint_in_progress());
        }
        Ok(())
    }

    /// Release a write slot (called when a write transaction commits or aborts).
    pub fn release_write(&self) {
        let prev = self.active_writes.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            // Last active write; wake up the checkpoint thread if waiting.
            self.condvar.notify_all();
        }
    }

    /// Pause new writes and wait for all active writes to complete.
    /// Returns Err if the timeout elapses before all writes drain.
    ///
    /// On timeout, writes are automatically resumed to prevent deadlock.
    pub fn pause_writes_and_drain(&self, timeout: Duration) -> Result<(), TransactionError> {
        self.writing_paused.store(true, Ordering::SeqCst);
        self.checkpoint_active.store(true, Ordering::SeqCst);
        self.in_storage_phase.store(false, Ordering::SeqCst);

        let mut guard = self.mutex.lock();
        let start = std::time::Instant::now();
        loop {
            let active = self.active_writes.load(Ordering::SeqCst);
            if active == 0 {
                return Ok(());
            }
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                self.writing_paused.store(false, Ordering::SeqCst);
                self.checkpoint_active.store(false, Ordering::SeqCst);
                return Err(TransactionError::checkpoint_timeout(active));
            }
            let remaining = timeout - elapsed;
            let result = self.condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                self.writing_paused.store(false, Ordering::SeqCst);
                self.checkpoint_active.store(false, Ordering::SeqCst);
                return Err(TransactionError::checkpoint_timeout(
                    self.active_writes.load(Ordering::SeqCst),
                ));
            }
        }
    }

    /// Resume accepting new write transactions after checkpoint completes.
    pub fn resume_writes(&self) {
        self.writing_paused.store(false, Ordering::SeqCst);
        self.checkpoint_active.store(false, Ordering::SeqCst);
        self.in_storage_phase.store(false, Ordering::SeqCst);
    }

    /// Early release of the write gate after WAL rotation.
    ///
    /// This mirrors Ladybug's optimisation where the WAL is frozen first and
    /// then writers are allowed to proceed while shadow pages and catalog
    /// serialization complete. The checkpoint remains logically active
    /// (checkpoint_active == true, in_storage_phase == true) even though
    /// new writes are no longer blocked.
    pub fn release_write_gate_early(&self) {
        self.writing_paused.store(false, Ordering::SeqCst);
        self.in_storage_phase.store(true, Ordering::SeqCst);
    }

    /// Complete the checkpoint's storage phase and clear active markers.
    pub fn complete_checkpoint(&self) {
        self.writing_paused.store(false, Ordering::SeqCst);
        self.checkpoint_active.store(false, Ordering::SeqCst);
        self.in_storage_phase.store(false, Ordering::SeqCst);
    }

    /// Whether writes are currently paused.
    pub fn is_paused(&self) -> bool {
        self.writing_paused.load(Ordering::SeqCst)
    }

    /// Whether a checkpoint is logically active (including storage phase).
    pub fn is_checkpoint_active(&self) -> bool {
        self.checkpoint_active.load(Ordering::SeqCst)
    }

    /// Whether checkpoint is in storage phase with early-released writes.
    pub fn is_in_storage_phase(&self) -> bool {
        self.in_storage_phase.load(Ordering::SeqCst)
    }

    /// Current count of active writes.
    pub fn active_write_count(&self) -> u64 {
        self.active_writes.load(Ordering::SeqCst)
    }
}

impl Default for CheckpointGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Shadow page manager for copy-on-write checkpoint semantics.
///
/// Shadow pages buffer dirty pages to a separate file during checkpoint's
/// storage phase. On success the shadow file is atomically swapped into place;
/// on failure it is discarded. This provides atomic checkpoint semantics even
/// when the write gate has been released early.
#[derive(Debug)]
pub struct ShadowPageManager {
    shadow_dir: std::path::PathBuf,
    shadow_files: parking_lot::Mutex<Vec<std::path::PathBuf>>,
}

impl ShadowPageManager {
    pub fn new(shadow_dir: impl Into<std::path::PathBuf>) -> Self {
        let dir = shadow_dir.into();
        let _ = std::fs::create_dir_all(&dir);
        Self {
            shadow_dir: dir,
            shadow_files: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn shadow_dir(&self) -> &std::path::Path {
        &self.shadow_dir
    }

    pub fn create_shadow_file(&self, seq: u64) -> std::io::Result<std::path::PathBuf> {
        let path = self.shadow_dir.join(format!("shadow_{:08x}.pages", seq));
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)?;
        self.shadow_files.lock().push(path.clone());
        Ok(path)
    }

    pub fn write_shadow_page(
        &self,
        shadow_file: &std::path::Path,
        page_id: u64,
        data: &[u8],
    ) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .open(shadow_file)?;
        file.seek(SeekFrom::Start(page_id * 4096))?;
        file.write_all(data)?;
        Ok(())
    }

    pub fn apply_shadow_pages(
        &self,
        shadow_file: &std::path::Path,
        target_file: &std::path::Path,
    ) -> std::io::Result<()> {
        if !shadow_file.exists() {
            return Ok(());
        }
        std::fs::copy(shadow_file, target_file)?;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(target_file)?;
        f.sync_all()?;
        Ok(())
    }

    pub fn cleanup_shadow_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        self.shadow_files.lock().retain(|p| p != path);
        Ok(())
    }

    pub fn cleanup_all(&self) -> usize {
        let mut count = 0;
        let files: Vec<_> = self.shadow_files.lock().clone();
        for f in files {
            if std::fs::remove_file(&f).is_ok() {
                count += 1;
            }
        }
        self.shadow_files.lock().clear();
        count
    }

    pub fn pending_shadow_files(&self) -> Vec<std::path::PathBuf> {
        self.shadow_files.lock().clone()
    }
}

/// Checkpoint transaction handle.
///
/// Held while a checkpoint is in progress. Writes are paused for the lifetime
/// of this handle (via [`CheckpointGate::pause_writes_and_drain`] at construction).
/// Once the caller finishes the checkpoint work, they must either [`commit`]
/// (writes a WAL `CheckpointMarker` record and resumes writes) or [`abort`]
/// (resumes writes without logging the marker). Dropping the handle also
/// resumes writes.
///
/// Phase 1 extends this with early write gate release: after WAL rotation the
/// caller may call `release_write_gate_early()` to resume writes while storage
/// work continues. The checkpoint transaction itself remains active until
/// `commit`/`abort`.
///
/// This mirrors Ladybug's `TRANSACTION_TYPE::CHECKPOINT` behavior: checkpoint
/// is an exclusive operation that prevents new writes from starting and drains
/// in-flight writes before proceeding.
pub struct CheckpointTransaction<'a> {
    manager: &'a TransactionManager,
    gate: Arc<CheckpointGate>,
    txn_id: TransactionId,
    write_ts: Timestamp,
    finished: bool,
    write_gate_released_early: bool,
}

impl CheckpointTransaction<'_> {
    /// Create a checkpoint handle bound to an in-progress checkpoint
    /// transaction. Only the transaction manager constructs this.
    pub(crate) fn new(
        manager: &TransactionManager,
        gate: Arc<CheckpointGate>,
        txn_id: TransactionId,
        write_ts: Timestamp,
    ) -> CheckpointTransaction<'_> {
        CheckpointTransaction {
            manager,
            gate,
            txn_id,
            write_ts,
            finished: false,
            write_gate_released_early: false,
        }
    }

    /// The write timestamp captured at checkpoint begin time.
    ///
    /// All data with `ts <= write_ts` is guaranteed to be visible at checkpoint.
    pub fn write_timestamp(&self) -> Timestamp {
        self.write_ts
    }

    /// Whether the write gate has been released early.
    pub fn is_write_gate_released_early(&self) -> bool {
        self.write_gate_released_early
    }

    /// Whether the checkpoint is in storage phase (writes resumed, checkpoint active).
    pub fn is_in_storage_phase(&self) -> bool {
        self.gate.is_in_storage_phase()
    }

    /// Release the write gate early after WAL rotation.
    ///
    /// After this call new write transactions may start while the checkpoint's
    /// storage phase (shadow pages, catalog serialization) continues. The
    /// checkpoint transaction itself remains active until `commit`/`abort`.
    pub fn release_write_gate_early(&mut self) {
        if !self.write_gate_released_early {
            self.gate.release_write_gate_early();
            self.write_gate_released_early = true;
        }
    }

    /// Complete the storage phase and commit the checkpoint.
    /// If writes were released early, this finalizes without re-pausing.
    pub fn commit(mut self) -> Result<(), TransactionError> {
        self.finished = true;
        let result = self.manager.commit_transaction(self.txn_id);
        self.gate.complete_checkpoint();
        result
    }

    /// Commit with early release: WAL rotation done, release writes, then
    /// run storage callback before final commit.
    pub fn commit_with_early_release<F>(mut self, storage_phase: F) -> Result<(), TransactionError>
    where
        F: FnOnce(Timestamp) -> Result<(), TransactionError>,
    {
        self.release_write_gate_early();
        let storage_result = storage_phase(self.write_ts);
        self.finished = true;
        if let Err(e) = storage_result {
            let _ = self.manager.abort_transaction(self.txn_id);
            self.gate.complete_checkpoint();
            return Err(e);
        }
        let result = self.manager.commit_transaction(self.txn_id);
        self.gate.complete_checkpoint();
        result
    }

    /// Abort the checkpoint without writing a WAL marker. Writes are resumed.
    pub fn abort(mut self) -> Result<(), TransactionError> {
        self.finished = true;
        let result = self.manager.abort_transaction(self.txn_id);
        self.gate.complete_checkpoint();
        result
    }
}

impl Drop for CheckpointTransaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.manager.abort_transaction(self.txn_id);
            self.gate.complete_checkpoint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TransactionErrorKind;
    use crate::types::{TransactionManagerConfig, TransactionOptions};

    #[test]
    fn checkpoint_gate_pauses_new_writes() {
        let gate = CheckpointGate::new();

        // Acquire a write slot.
        assert!(gate.acquire_write().is_ok());
        assert_eq!(gate.active_write_count(), 1);

        // Pause and drain.
        gate.writing_paused.store(true, Ordering::SeqCst);

        // New writes should fail.
        assert!(gate.acquire_write().is_err());

        // Release the active write.
        gate.release_write();

        // Still paused, should fail.
        assert!(gate.acquire_write().is_err());

        // Resume.
        gate.resume_writes();
        assert!(!gate.is_paused());
        assert!(gate.acquire_write().is_ok());
        assert_eq!(gate.active_write_count(), 1);
    }

    #[test]
    fn checkpoint_gate_drain_waits_for_active_writes() {
        use std::thread;

        let gate = Arc::new(CheckpointGate::new());
        let gate_clone = Arc::clone(&gate);

        // Start a write transaction.
        gate.acquire_write().expect("acquire should succeed");

        // Spawn a thread that will release after a short delay.
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            gate_clone.release_write();
        });

        // Drain should wait until the write is released.
        let result = gate.pause_writes_and_drain(Duration::from_secs(5));
        assert!(result.is_ok());
        assert_eq!(gate.active_write_count(), 0);

        handle.join().unwrap();
    }

    #[test]
    fn checkpoint_gate_drain_timeout() {
        let gate = CheckpointGate::new();

        // Acquire a write slot and never release.
        gate.acquire_write().expect("acquire should succeed");

        // Drain with short timeout should fail.
        let result = gate.pause_writes_and_drain(Duration::from_millis(50));
        assert!(result.is_err());
    }

    #[test]
    fn begin_insert_fails_when_checkpoint_paused() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        // Pause writes via checkpoint.
        manager
            .checkpoint_gate()
            .writing_paused
            .store(true, Ordering::SeqCst);

        // New insert should fail.
        let result = manager.begin_insert_transaction(TransactionOptions::default());
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind(),
            TransactionErrorKind::CheckpointInProgress
        );

        // Read transactions should still work.
        let read_result = manager.begin_read_transaction(TransactionOptions::default());
        assert!(read_result.is_ok());

        // Resume writes.
        manager.end_checkpoint();
        let insert_result = manager.begin_insert_transaction(TransactionOptions::default());
        assert!(insert_result.is_ok());
    }

    #[test]
    fn coordinated_checkpoint_drains_writes() {
        use std::thread;

        let manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));
        let manager_clone = Arc::clone(&manager);

        // Start a write transaction.
        let txn_id = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("transaction should begin");

        // Spawn a thread that commits the transaction after a delay.
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            manager_clone
                .commit_transaction(txn_id)
                .expect("commit should succeed");
        });

        // Coordinated checkpoint should wait for the write to complete.
        let result = manager.coordinated_checkpoint(Duration::from_secs(5), |_ts| {
            Ok(graphdb_core::wal::types::Lsn::new(100))
        });
        assert!(result.is_ok());

        handle.join().unwrap();
    }

    #[test]
    fn checkpoint_transaction_drain_active_writes() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let write_txn = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("write should begin");

        let manager2 = Arc::new(manager);
        let mgr_clone = Arc::clone(&manager2);

        // Spawn a thread that tries to begin a checkpoint transaction.
        // It should block until the write commits/times out.
        let handle = std::thread::spawn(move || {
            let checkpoint = mgr_clone
                .begin_checkpoint_transaction(Duration::from_secs(5))
                .expect("checkpoint should begin after drain");
            assert!(checkpoint.write_timestamp() > 0);
            checkpoint.commit().expect("checkpoint should commit");
        });

        // Give the checkpoint thread a moment to start draining.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Commit the active write so the checkpoint can proceed.
        manager2
            .commit_transaction(write_txn)
            .expect("write should commit");

        handle.join().unwrap();

        // Verify writes are resumed after checkpoint commit.
        let gate = manager2.checkpoint_gate();
        assert!(!gate.is_paused());
        assert_eq!(gate.active_write_count(), 0);
    }

    #[test]
    fn checkpoint_transaction_abort_resumes_writes() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        let checkpoint = manager
            .begin_checkpoint_transaction(Duration::from_secs(5))
            .expect("checkpoint should begin");
        assert!(manager.checkpoint_gate().is_paused());

        // Abort resumes writes.
        checkpoint.abort().expect("checkpoint should abort");
        assert!(!manager.checkpoint_gate().is_paused());
    }

    #[test]
    fn checkpoint_transaction_is_monitored_and_emits_commit() {
        use std::sync::atomic::AtomicUsize;

        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let commits = Arc::new(AtomicUsize::new(0));
        let commit_count = Arc::clone(&commits);
        manager.register_commit_callback(Arc::new(move |event| {
            if let crate::types::TransactionEvent::Committed { .. } = event {
                commit_count.fetch_add(1, Ordering::SeqCst);
            }
        }));

        let checkpoint = manager
            .begin_checkpoint_transaction(Duration::from_secs(5))
            .expect("checkpoint should begin");
        let transactions = manager.list_transactions();
        assert_eq!(transactions.len(), 1);
        assert_eq!(
            transactions[0].txn_type,
            crate::types::TransactionType::Checkpoint
        );

        checkpoint.commit().expect("checkpoint should commit");
        assert!(manager.list_transactions().is_empty());
        assert_eq!(commits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn checkpoint_transaction_drop_resumes_writes() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());

        {
            let checkpoint = manager
                .begin_checkpoint_transaction(Duration::from_secs(5))
                .expect("checkpoint should begin");
            assert!(manager.checkpoint_gate().is_paused());
            // Drop without commit — should resume writes.
            drop(checkpoint);
        }
        assert!(!manager.checkpoint_gate().is_paused());
    }

    #[test]
    fn checkpoint_early_release_allows_writes_during_storage_phase() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let mut checkpoint = manager
            .begin_checkpoint_transaction(Duration::from_secs(5))
            .expect("checkpoint should begin");
        assert!(manager.checkpoint_gate().is_paused());
        assert!(!manager.checkpoint_gate().is_in_storage_phase());

        checkpoint.release_write_gate_early();
        assert!(!manager.checkpoint_gate().is_paused());
        assert!(manager.checkpoint_gate().is_in_storage_phase());
        assert!(manager.checkpoint_gate().is_checkpoint_active());
        assert!(checkpoint.is_write_gate_released_early());

        // New writes should succeed after early release.
        let txn = manager
            .begin_insert_transaction(TransactionOptions::default())
            .expect("write should succeed after early release");
        manager
            .commit_transaction(txn)
            .expect("commit should succeed");

        checkpoint.commit().expect("checkpoint should commit");
        assert!(!manager.checkpoint_gate().is_checkpoint_active());
        assert!(!manager.checkpoint_gate().is_in_storage_phase());
    }

    #[test]
    fn coordinated_checkpoint_with_early_release_releases_after_wal_phase() {
        let manager = TransactionManager::new(TransactionManagerConfig::default());
        let result = manager.coordinated_checkpoint_with_early_release(
            Duration::from_secs(5),
            |ts| {
                assert!(ts > 0);
                Ok(graphdb_core::wal::types::Lsn::new(200))
            },
            |ts, lsn| {
                assert!(ts > 0);
                assert_eq!(lsn, graphdb_core::wal::types::Lsn::new(200));
                // Storage phase should have writes released.
                assert!(!manager.checkpoint_gate().is_paused());
                assert!(manager.checkpoint_gate().is_in_storage_phase());
                Ok(())
            },
        );
        assert!(result.is_ok());
        assert!(!manager.checkpoint_gate().is_checkpoint_active());
    }

    #[test]
    fn shadow_page_manager_creates_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ShadowPageManager::new(dir.path());
        let shadow = mgr.create_shadow_file(1).unwrap();
        assert!(shadow.exists());
        mgr.write_shadow_page(&shadow, 0, &[1u8; 4096]).unwrap();
        assert_eq!(mgr.pending_shadow_files().len(), 1);
        mgr.cleanup_shadow_file(&shadow).unwrap();
        assert!(!shadow.exists());
        assert!(mgr.pending_shadow_files().is_empty());
    }
}
