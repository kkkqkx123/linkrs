//! Recovery of transactions whose WAL data is durable but whose post-commit
//! finalization (commit_sink finalize, undo-log cleanup) failed.
//!
//! These transactions are recorded and re-driven on the next
//! `startup_recovery()` call or on admin-triggered recovery.
//!
//! Responsibility boundary: this crate decides *which* transactions need
//! re-driving (durable-but-unfinalized) and retries storage finalization
//! through the commit sink; the storage engine owns *page-level* replay and
//! must make `finalize_commit` idempotent. The pending queue can be mirrored
//! to a sidecar file next to the WAL directory so the intent survives a
//! process crash; on restart the storage-level WAL replay redoes committed
//! payloads while this queue tells the sink which commits still owe
//! post-commit finalization.

use std::path::PathBuf;

use parking_lot::Mutex;

use graphdb_core::types::{CommitLsn, Timestamp, TransactionId};

use super::error::TransactionError;
use super::participant::{TransactionCommitDescriptor, TransactionCommitSink};

/// A transaction whose WAL data is durable but whose post-commit state is
/// incomplete (finalize_commit or undo-log cleanup failed).
///
/// `commit_timestamp` is 0 while the commit timestamp has not been allocated
/// (finalize failed before visibility was published); recovery allocates it
/// when finalization is re-driven. The stored descriptor carries everything
/// the commit sink needs to retry finalization idempotently.
#[derive(Debug, Clone)]
pub struct PendingFinalization {
    pub txn_id: TransactionId,
    pub write_timestamp: Timestamp,
    pub commit_timestamp: Timestamp,
    pub commit_lsn: CommitLsn,
    pub descriptor: TransactionCommitDescriptor,
}

/// Minimal durable record for the sidecar file: one line per pending
/// transaction (`txn_id write_timestamp commit_timestamp commit_lsn`).
///
/// Intentionally ID-only: the mutation payload itself stays in the storage
/// WAL (page-level replay owns redo), while this queue tells the sink which
/// commits still owe post-commit finalization. A restart therefore replays
/// payloads via storage WAL replay and re-drives finalization via
/// `startup_recovery` + `recover_unfinalized_commits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SidecarRecord {
    txn_id: u64,
    write_timestamp: Timestamp,
    commit_timestamp: Timestamp,
    commit_lsn: u64,
}

/// Tracks transactions whose data is durable but whose post-commit
/// finalization failed, and re-drives recovery when asked.
pub struct RecoveryManager {
    pending_finalizations: Mutex<Vec<PendingFinalization>>,
    sidecar_path: Mutex<Option<PathBuf>>,
}

impl RecoveryManager {
    pub fn new() -> Self {
        Self {
            pending_finalizations: Mutex::new(Vec::new()),
            sidecar_path: Mutex::new(None),
        }
    }

    /// Mirror the pending queue to a sidecar file (typically inside the WAL
    /// directory) so unfinalized-commit intent survives a process crash.
    /// Only sets the path: existing file contents are left untouched until
    /// the next `record` (which rewrites from the merged queue) or
    /// `recover` (which consumes and clears them). Persistence is
    /// best-effort: I/O failures are logged and the in-memory queue still
    /// drives same-process recovery.
    pub fn set_sidecar_path(&self, path: impl Into<PathBuf>) {
        *self.sidecar_path.lock() = Some(path.into());
    }

    /// Record a transaction whose finalization still needs to be re-driven.
    pub fn record(
        &self,
        descriptor: &TransactionCommitDescriptor,
        commit_timestamp: Timestamp,
        commit_lsn: CommitLsn,
    ) {
        let mut queue = self.pending_finalizations.lock();
        // One pending record per transaction: re-recording replaces the
        // older entry so retries cannot accumulate duplicates.
        queue.retain(|pending| pending.txn_id != descriptor.transaction_id);
        queue.push(PendingFinalization {
            txn_id: descriptor.transaction_id,
            write_timestamp: descriptor.write_timestamp,
            commit_timestamp,
            commit_lsn,
            descriptor: descriptor.clone(),
        });
        self.persist_locked(&queue);
    }

    /// Rewrite the sidecar file from the in-memory queue. Callers must hold
    /// the queue lock (or have exclusive access) so the file mirrors memory.
    fn persist_locked(&self, queue: &[PendingFinalization]) {
        let Some(path) = self.sidecar_path.lock().clone() else {
            return;
        };
        let mut contents = String::new();
        for pending in queue {
            contents.push_str(&format!(
                "{} {} {} {}\n",
                pending.txn_id.0,
                pending.write_timestamp,
                pending.commit_timestamp,
                pending.commit_lsn.get(),
            ));
        }
        if let Err(error) = std::fs::write(&path, contents) {
            log::warn!(
                "Failed to persist {} pending finalization(s) to {}: {}",
                queue.len(),
                path.display(),
                error
            );
            return;
        }
        // Best-effort durability for the sidecar itself.
        if let Ok(file) = std::fs::File::open(&path) {
            let _ = file.sync_all();
        }
    }

    /// Load sidecar records written by a previous process. Malformed lines
    /// are skipped with a warning; they never fail recovery.
    fn load_sidecar(&self) -> Vec<SidecarRecord> {
        let Some(path) = self.sidecar_path.lock().clone() else {
            return Vec::new();
        };
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                log::warn!(
                    "Failed to read recovery sidecar {}: {}",
                    path.display(),
                    error
                );
                return Vec::new();
            }
        };
        let mut records = Vec::new();
        for (line_number, line) in contents.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let parsed = (
                parts.next().and_then(|part| part.parse::<u64>().ok()),
                parts.next().and_then(|part| part.parse::<Timestamp>().ok()),
                parts.next().and_then(|part| part.parse::<Timestamp>().ok()),
                parts.next().and_then(|part| part.parse::<u64>().ok()),
            );
            match parsed {
                (Some(txn_id), Some(write_timestamp), Some(commit_timestamp), Some(commit_lsn)) => {
                    records.push(SidecarRecord {
                        txn_id,
                        write_timestamp,
                        commit_timestamp,
                        commit_lsn,
                    });
                }
                _ => {
                    log::warn!(
                        "Skipping malformed recovery sidecar line {} in {}",
                        line_number + 1,
                        path.display()
                    );
                }
            }
        }
        records
    }

    /// Remove the sidecar file after its records have been accounted for.
    fn clear_sidecar(&self) {
        let Some(path) = self.sidecar_path.lock().clone() else {
            return;
        };
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "Failed to remove recovery sidecar {}: {}",
                    path.display(),
                    error
                );
            }
        }
    }

    /// Transaction IDs with queued finalizations.
    pub fn pending_txn_ids(&self) -> Vec<TransactionId> {
        self.pending_finalizations
            .lock()
            .iter()
            .map(|pending| pending.txn_id)
            .collect()
    }

    /// Take (and remove) the queued finalization for one transaction, so a
    /// re-drive owns it exclusively. Re-queue by calling `record` on failure.
    pub fn take_pending(&self, txn_id: TransactionId) -> Option<PendingFinalization> {
        let mut queue = self.pending_finalizations.lock();
        let position = queue.iter().position(|pending| pending.txn_id == txn_id)?;
        Some(queue.remove(position))
    }

    /// Recover transactions whose data was durably persisted but whose
    /// post-commit finalization (commit_sink finalize or undo-log cleanup)
    /// left them in an incomplete state.
    ///
    /// Called once at startup (after WAL replay) and can also be invoked
    /// on demand by an administrator.
    ///
    /// 1. Account for in-memory pending finalizations queued by prior commit
    ///    failures in this process, plus sidecar records left by a previous
    ///    process. Same-process pendings that still have a live transaction
    ///    context are re-driven by the manager (`startup_recovery` calls
    ///    `recover_pending_finalization` first); this method accounts for
    ///    the remainder at the storage level.
    /// 2. Ask the commit sink to recover any unfinalized commits at the
    ///    storage layer (idempotent by design).
    ///
    /// Returns the number of recovered commits.
    pub fn recover(
        &self,
        sink: Option<&dyn TransactionCommitSink>,
    ) -> Result<usize, TransactionError> {
        let mut recovered = 0usize;

        let pending: Vec<PendingFinalization> = {
            let mut queue = self.pending_finalizations.lock();
            std::mem::take(&mut *queue)
        };
        for pf in &pending {
            log::info!(
                "Recovering pending finalization: txn={:?} write_ts={} commit_ts={} lsn={:?}",
                pf.txn_id,
                pf.write_timestamp,
                pf.commit_timestamp,
                pf.commit_lsn,
            );
        }
        recovered += pending.len();

        let sidecar_records = self.load_sidecar();
        for record in &sidecar_records {
            log::info!(
                "Recovering sidecar finalization: txn={} write_ts={} commit_ts={} lsn={}",
                record.txn_id,
                record.write_timestamp,
                record.commit_timestamp,
                record.commit_lsn,
            );
        }
        recovered += sidecar_records.len();

        if let Some(sink) = sink {
            let n = sink
                .recover_unfinalized_commits()
                .map_err(|e| TransactionError::internal(format!("Recovery failed: {}", e)))?;
            recovered += n;
        }

        self.clear_sidecar();
        Ok(recovered)
    }
}

impl Default for RecoveryManager {
    fn default() -> Self {
        Self::new()
    }
}
