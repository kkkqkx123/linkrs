//! Transaction context behavior: savepoints, undo logs and modified table tracking

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::TransactionContext;
use crate::error::TransactionError;
use crate::types::*;
use crate::undo_log::{UndoLogEntry, UndoTarget};
use graphdb_core::types::StagedWriteMark;
/// Savepoint Manager
pub(crate) struct SavepointManager {
    savepoints: HashMap<SavepointId, SavepointInfo>,
    next_id: SavepointId,
    next_sequence: u64,
}

impl SavepointManager {
    pub(super) fn new() -> Self {
        Self {
            savepoints: HashMap::new(),
            next_id: 1,
            next_sequence: 1,
        }
    }

    pub(super) fn create_savepoint(&mut self, params: SavepointParams) -> SavepointId {
        let id = self.next_id;
        self.next_id += 1;
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let info = SavepointInfo {
            id,
            name: params.name,
            created_at: Instant::now(),
            sequence,
            undo_log_index: params.undo_log_index,
            sync_sequence: params.sync_sequence,
            write_set: params.write_set,
            read_set: params.read_set,
            modified_tables: params.modified_tables,
            journal_len: params.journal_len,
            journal_next_sequence: params.journal_next_sequence,
            staged_write_mark: None,
        };
        self.savepoints.insert(id, info);
        id
    }

    pub(super) fn set_staging_mark(
        &mut self,
        id: SavepointId,
        mark: Option<StagedWriteMark>,
    ) -> Result<(), TransactionError> {
        let savepoint = self
            .savepoints
            .get_mut(&id)
            .ok_or(TransactionError::savepoint_not_found(id))?;
        savepoint.staged_write_mark = mark;
        Ok(())
    }

    pub(super) fn get_savepoint(&self, id: SavepointId) -> Option<&SavepointInfo> {
        self.savepoints.get(&id)
    }

    pub(super) fn remove_savepoint(&mut self, id: SavepointId) -> Option<SavepointInfo> {
        self.savepoints.remove(&id)
    }

    pub(super) fn clear(&mut self) {
        self.savepoints.clear();
    }

    pub(super) fn find_by_name(&self, name: &str) -> Option<SavepointInfo> {
        self.savepoints
            .values()
            .filter(|sp| sp.name.as_deref() == Some(name))
            .max_by_key(|sp| sp.sequence)
            .cloned()
    }
}

impl TransactionContext {
    /// Record table modification
    pub fn record_table_modification(&self, table_name: &str) {
        let mut tables = self.modified_tables.lock();
        if !tables.contains(&table_name.to_string()) {
            tables.push(table_name.to_string());
        }
    }

    /// Get modified tables
    pub fn get_modified_tables(&self) -> Vec<String> {
        let tables = self.modified_tables.lock();
        tables.clone()
    }
    /// Create savepoint
    ///
    /// The savepoint boundary is the canonical journal length; derived views
    /// (redo cache, local WAL buffer) are rebuilt from the journal on
    /// rollback, so no derived offsets are captured here.
    pub fn create_savepoint(&self, name: Option<String>, sync_sequence: u64) -> SavepointId {
        let (journal_len, journal_next) = {
            let j = self.mutation_journal.read();
            (j.len(), j.next_sequence())
        };
        let params = SavepointParams {
            name,
            undo_log_index: self.undo_log_len(),
            sync_sequence,
            write_set: self.get_write_set(),
            read_set: self.get_read_set(),
            modified_tables: self.get_modified_tables(),
            journal_len,
            journal_next_sequence: journal_next,
        };
        let mut manager = self.savepoint_manager.write();
        manager.create_savepoint(params)
    }

    /// Attach the storage-staged write boundary captured at savepoint
    /// creation. The transaction layer cannot observe the storage staging
    /// buffer, so the savepoint creator (session / service / test harness)
    /// records the mark it peeked from the undo target here.
    pub fn set_savepoint_staging_mark(
        &self,
        id: SavepointId,
        mark: Option<StagedWriteMark>,
    ) -> Result<(), TransactionError> {
        let mut manager = self.savepoint_manager.write();
        manager.set_staging_mark(id, mark)
    }

    /// Get savepoint info
    pub fn get_savepoint(&self, id: SavepointId) -> Option<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.get_savepoint(id).cloned()
    }

    /// Find savepoint by ID (alias for get_savepoint for API clarity)
    pub fn find_savepoint_by_id(&self, id: SavepointId) -> Option<SavepointInfo> {
        self.get_savepoint(id)
    }

    /// Get all savepoints
    pub fn get_all_savepoints(&self) -> Vec<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.savepoints.values().cloned().collect()
    }

    /// Find savepoint by name
    pub fn find_savepoint_by_name(&self, name: &str) -> Option<SavepointInfo> {
        let manager = self.savepoint_manager.read();
        manager.find_by_name(name)
    }

    /// Release savepoint
    pub fn release_savepoint(&self, id: SavepointId) -> Result<(), TransactionError> {
        let mut manager = self.savepoint_manager.write();
        manager
            .remove_savepoint(id)
            .map(|_| ())
            .ok_or(TransactionError::savepoint_not_found(id))
    }

    /// Rollback to savepoint
    pub fn rollback_to_savepoint<T: UndoTarget + ?Sized>(
        &self,
        id: SavepointId,
        target: &T,
    ) -> Result<(), TransactionError> {
        let state = self.state.load();
        if !state.can_execute() {
            return Err(TransactionError::invalid_state_for_abort(state));
        }

        if self.is_expired() {
            return Err(TransactionError::transaction_expired());
        }

        let savepoint_info = {
            let manager = self.savepoint_manager.read();
            manager
                .get_savepoint(id)
                .cloned()
                .ok_or(TransactionError::savepoint_not_found(id))?
        };

        // Validate journal position is the authoritative savepoint boundary.
        {
            let journal = self.mutation_journal.read();
            let position = crate::mutation_journal::MutationJournalPosition {
                journal_len: savepoint_info.journal_len,
                next_sequence: savepoint_info.journal_next_sequence,
                undo_log_index: savepoint_info.undo_log_index,
                modified_tables: savepoint_info.modified_tables.clone(),
                write_set_snapshot: savepoint_info.write_set.clone(),
                read_set_snapshot: savepoint_info.read_set.clone(),
                sync_sequence: savepoint_info.sync_sequence,
                savepoint_sequence: savepoint_info.sequence,
            };
            if let Err(e) = position.validate_against(&journal) {
                return Err(TransactionError::rollback_failed(format!(
                    "savepoint journal invariant violated: {}",
                    e
                )));
            }
        }

        // Use undo rollback for savepoint

        {
            let mut manager = self.savepoint_manager.write();
            // Delete savepoints created AFTER the target savepoint using
            // explicit sequence number (not ID). This ensures stable ordering
            // even if IDs are not assigned in strict creation order.
            let target_sequence = savepoint_info.sequence;
            let savepoints_to_remove: Vec<SavepointId> = manager
                .savepoints
                .iter()
                .filter(|(_, sp)| sp.sequence > target_sequence)
                .map(|(&id, _)| id)
                .collect();

            for sp_id in savepoints_to_remove {
                manager.remove_savepoint(sp_id);
            }
        }

        self.execute_undo_logs_from_index(target, savepoint_info.undo_log_index)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;

        // Staged (not yet undo-logged) writes rewind to the savepoint mark.
        // Without this, rows staged after the savepoint stay visible inside
        // the transaction after ROLLBACK TO.
        if let Some(mark) = savepoint_info.staged_write_mark {
            target
                .rollback_staged_writes(self.id, mark)
                .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;
        }

        self.restore_write_set(savepoint_info.write_set);
        self.restore_read_set(savepoint_info.read_set);
        {
            let mut tables = self.modified_tables.lock();
            *tables = savepoint_info.modified_tables.clone();
        }
        {
            let mut journal = self.mutation_journal.write();
            if savepoint_info.journal_len > journal.len() {
                return Err(TransactionError::rollback_failed(
                    "savepoint journal length exceeds current journal",
                ));
            }
            journal.truncate(savepoint_info.journal_len);
            if cfg!(debug_assertions) {
                if let Err(e) = journal.check_invariants() {
                    log::error!("journal invariant after savepoint rollback: {}", e);
                }
            }
        }
        // Derived views mirror the truncated journal.
        self.rebuild_derived_logs();
        // Recompute mutation count and undo bytes from remaining journal to keep
        // budgets consistent after truncation.
        {
            let journal = self.mutation_journal.read();
            self.mutation_count
                .store(journal.len() as u64, Ordering::Relaxed);
            // undo_bytes is estimated; reset proportionally to remaining entries
            let remaining = journal.len() as u64 * 64;
            self.undo_bytes.store(remaining, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Add undo log
    pub fn add_undo_log(&self, log: UndoLogEntry) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .add(log)
            .map_err(|error| TransactionError::internal(error.to_string()))
    }

    /// Get undo log length
    pub fn undo_log_len(&self) -> usize {
        let undo_logs = self.undo_logs.read();
        undo_logs.len()
    }

    /// Clear undo logs
    pub fn clear_undo_logs(&self) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .clear()
            .map_err(|error| TransactionError::internal(error.to_string()))
    }

    /// Execute undo logs for rollback
    pub fn execute_undo_logs<T: UndoTarget + ?Sized>(
        &self,
        target: &T,
    ) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .execute_undo(target, self.start_timestamp)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))
    }

    /// Execute undo logs starting from a specific index.
    pub fn execute_undo_logs_from_index<T: UndoTarget + ?Sized>(
        &self,
        target: &T,
        start_index: usize,
    ) -> Result<(), TransactionError> {
        let mut undo_logs = self.undo_logs.write();
        undo_logs
            .execute_undo_from_index(target, self.start_timestamp, start_index)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))
    }
}
