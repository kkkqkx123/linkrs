//! Transaction checkpoint coordination

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::TransactionManager;
use crate::checkpoint::CheckpointTransaction;
use crate::context::TransactionContext;
use crate::error::TransactionError;
use crate::types::*;
use graphdb_core::types::Timestamp;
use graphdb_core::wal::types::Lsn;

impl TransactionManager {
    /// Begin a checkpoint operation: pause new writes and wait for active
    /// write transactions to complete.
    ///
    /// This is the first phase of a checkpoint. After this returns Ok,
    /// no new write transactions can start and all previously active writes
    /// have been drained (committed or aborted).
    ///
    /// # Arguments
    /// - `timeout`: Maximum time to wait for active writes to drain.
    ///
    /// # Returns
    /// - `Ok(())` if writes are paused and all active writes have drained.
    /// - `Err(TransactionError::CheckpointTimeout)` if timeout elapses first.
    pub fn begin_checkpoint(&self, timeout: Duration) -> Result<(), TransactionError> {
        if self.config.in_memory {
            return Ok(());
        }
        self.checkpoint_gate.pause_writes_and_drain(timeout)
    }

    /// End a checkpoint operation: resume accepting new write transactions.
    ///
    /// This must be called after `begin_checkpoint` completes, regardless
    /// of whether the checkpoint itself succeeded or failed.
    pub fn end_checkpoint(&self) {
        if self.config.in_memory {
            return;
        }
        self.checkpoint_gate.resume_writes()
    }

    /// Execute a coordinated checkpoint: pause writes, run the provided
    /// callback, then resume writes.
    ///
    /// The callback receives the current write timestamp and should return
    /// the LSN to checkpoint at. Writes are guaranteed to be paused for
    /// the duration of the callback.
    ///
    /// If the callback returns Err, writes are still resumed.
    pub fn coordinated_checkpoint<F>(
        &self,
        timeout: Duration,
        f: F,
    ) -> Result<(Timestamp, Lsn), TransactionError>
    where
        F: FnOnce(Timestamp) -> Result<Lsn, TransactionError>,
    {
        if self.config.in_memory {
            let write_ts = self.version_manager.write_timestamp();
            let lsn = f(write_ts)?;
            return Ok((write_ts, lsn));
        }
        let checkpoint = self.begin_checkpoint_transaction(timeout)?;
        let write_ts = checkpoint.write_timestamp();
        match f(write_ts) {
            Ok(lsn) => {
                checkpoint.commit()?;
                Ok((write_ts, lsn))
            }
            Err(error) => {
                if let Err(abort_error) = checkpoint.abort() {
                    log::error!("Checkpoint rollback failed: {}", abort_error);
                }
                Err(error)
            }
        }
    }

    /// Execute a coordinated checkpoint with early write gate release.
    ///
    /// Phase 1 (WAL rotation) runs with writes paused; after `wal_phase`
    /// returns, the write gate is released early via
    /// `CheckpointGate::release_write_gate_early` so that new writers can
    /// proceed while `storage_phase` (shadow pages / catalog serialization)
    /// completes. The checkpoint transaction remains active until the final
    /// commit.
    pub fn coordinated_checkpoint_with_early_release<WF, SF>(
        &self,
        timeout: Duration,
        wal_phase: WF,
        storage_phase: SF,
    ) -> Result<(Timestamp, Lsn), TransactionError>
    where
        WF: FnOnce(Timestamp) -> Result<Lsn, TransactionError>,
        SF: FnOnce(Timestamp, Lsn) -> Result<(), TransactionError>,
    {
        if self.config.in_memory {
            let write_ts = self.version_manager.write_timestamp();
            let lsn = wal_phase(write_ts)?;
            storage_phase(write_ts, lsn)?;
            return Ok((write_ts, lsn));
        }
        let mut checkpoint = self.begin_checkpoint_transaction(timeout)?;
        let write_ts = checkpoint.write_timestamp();
        let lsn = match wal_phase(write_ts) {
            Ok(lsn) => lsn,
            Err(e) => {
                if let Err(abort_error) = checkpoint.abort() {
                    log::error!("Checkpoint WAL phase rollback failed: {}", abort_error);
                }
                return Err(e);
            }
        };
        checkpoint.release_write_gate_early();
        if let Err(e) = storage_phase(write_ts, lsn) {
            if let Err(abort_error) = checkpoint.abort() {
                log::error!("Checkpoint storage phase rollback failed: {}", abort_error);
            }
            return Err(e);
        }
        checkpoint.commit()?;
        Ok((write_ts, lsn))
    }

    /// Begin a checkpoint transaction.
    ///
    /// Returns a [`CheckpointTransaction`] handle that keeps writes paused for
    /// its lifetime. The caller performs checkpoint work (e.g. via
    /// [`CheckpointManager`]), then either calls `commit()` or `abort()` on the
    /// handle. Dropping the handle aborts the checkpoint (resumes writes).
    ///
    /// Unlike `coordinated_checkpoint`, this does not run a callback — it
    /// gives the caller full control over the checkpoint lifecycle.
    ///
    /// In-memory mode semantics: the checkpoint is a no-op for durability
    /// (no WAL, no write gate), but the transaction still captures a snapshot
    /// timestamp and is recorded in monitoring, so observability stays
    /// consistent across modes.
    ///
    /// # Errors
    /// Returns `Err(TransactionError::CheckpointTimeout)` if active writes
    /// do not drain within `timeout`.
    pub fn begin_checkpoint_transaction(
        &self,
        timeout: Duration,
    ) -> Result<CheckpointTransaction<'_>, TransactionError> {
        if self.shutdown_flag.load(Ordering::SeqCst) != 0 {
            return Err(TransactionError::internal(
                "Transaction manager is shutdown".to_string(),
            ));
        }
        if self.active_transactions.len() >= self.config.max_concurrent_transactions {
            return Err(TransactionError::too_many_transactions());
        }

        self.reset_checkpoint_commit_counter();
        if !self.config.in_memory {
            self.checkpoint_gate.pause_writes_and_drain(timeout)?;
        }
        let write_ts = self.version_manager.write_timestamp();
        let txn_id = TransactionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let context = Arc::new(TransactionContext::new_checkpoint(
            txn_id,
            write_ts,
            self.config.txn_config.clone(),
        ));
        self.active_transactions.insert(txn_id, context);
        self.stats.record_txn_begin();

        Ok(CheckpointTransaction::new(
            self,
            Arc::clone(&self.checkpoint_gate),
            txn_id,
            write_ts,
        ))
    }
}
