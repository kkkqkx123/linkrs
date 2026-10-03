//! Durability decision paths: group commit, buffered-flush fsync and plain
//! file fsync, shared by `sync()` and `wait_for_durable()`.

use std::sync::atomic::Ordering;
use std::time::Instant;

use graphdb_core::wal::types::{WalError, WalResult};

use super::LocalWalWriter;

impl LocalWalWriter {
    /// File-level sync shared by `sync()` after buffers are drained.
    pub(crate) fn sync_via_file(&self) -> WalResult<()> {
        self.check_poisoned()?;
        let current_lsn = self.current_lsn.load(Ordering::SeqCst);
        if let Some(ref coordinator) = self.group_commit {
            coordinator.record_appended(current_lsn);
            coordinator.append_and_wait(current_lsn)?;
        } else if self.flush_state.is_some() {
            // Buffered mode without group commit: fsync via shared handle.
            if let Some(ref state) = self.flush_state {
                let guard = state.lock().map_err(|e| {
                    WalError::InvalidOperation(format!("flush state lock poisoned: {}", e))
                })?;
                guard.file.sync_all().map_err(|e| {
                    self.poison(format!("fsync failed: {}", e));
                    WalError::IoError(e.to_string())
                })?;
            }
        } else if let Some(ref file) = self.file {
            if let Err(e) = file.sync_all() {
                self.poison(format!("fsync failed: {}", e));
                return Err(WalError::IoError(e.to_string()));
            }
        }
        self.last_synced_lsn.store(current_lsn, Ordering::SeqCst);
        self.write_count.store(0, Ordering::SeqCst);
        if let Ok(mut guard) = self.last_sync_time.lock() {
            *guard = Some(Instant::now());
        }
        Ok(())
    }

    /// Durability wait after buffers are drained: group commit, shared
    /// buffered handle fsync, or plain file fsync.
    pub(crate) fn wait_durable_via_file(&self, appended_lsn: u64) -> WalResult<()> {
        if let Some(ref coordinator) = self.group_commit {
            coordinator.record_appended(appended_lsn);
            coordinator.append_and_wait(appended_lsn)
        } else if self.flush_state.is_some() {
            self.check_poisoned()?;
            if let Some(ref state) = self.flush_state {
                let guard = state.lock().map_err(|e| {
                    WalError::InvalidOperation(format!("flush state lock poisoned: {}", e))
                })?;
                guard
                    .file
                    .sync_all()
                    .map_err(|e| WalError::IoError(e.to_string()))?;
            }
            self.last_synced_lsn.store(appended_lsn, Ordering::SeqCst);
            Ok(())
        } else if let Some(ref file) = self.file {
            self.check_poisoned()?;
            file.sync_all()
                .map_err(|e| WalError::IoError(e.to_string()))?;
            self.last_synced_lsn.store(appended_lsn, Ordering::SeqCst);
            Ok(())
        } else {
            Err(WalError::Closed)
        }
    }
}
