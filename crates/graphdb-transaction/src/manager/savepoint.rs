//! Transaction savepoint management

use super::TransactionManager;
use crate::error::TransactionError;
use crate::types::*;
use crate::undo_log::UndoTarget;

impl TransactionManager {
    /// Create savepoint
    pub fn create_savepoint(
        &self,
        txn_id: TransactionId,
        name: Option<String>,
    ) -> Result<SavepointId, TransactionError> {
        let context = self.get_context(txn_id)?;
        let sync_sequence = self
            .sync_manager
            .as_ref()
            .map(|manager| manager.pending_transaction_intent_sequence(txn_id))
            .unwrap_or(0);
        Ok(context.create_savepoint(name, sync_sequence))
    }

    /// Get savepoint info
    pub fn get_savepoint(&self, txn_id: TransactionId, id: SavepointId) -> Option<SavepointInfo> {
        let context = self.get_context(txn_id).ok()?;
        context.get_savepoint(id)
    }

    /// Release savepoint
    pub fn release_savepoint(
        &self,
        txn_id: TransactionId,
        id: SavepointId,
    ) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        context.release_savepoint(id)
    }

    /// Rollback to savepoint
    pub fn rollback_to_savepoint<T: UndoTarget + ?Sized>(
        &self,
        txn_id: TransactionId,
        id: SavepointId,
        target: &T,
    ) -> Result<(), TransactionError> {
        let context = self.get_context(txn_id)?;
        let savepoint = context
            .get_savepoint(id)
            .ok_or(TransactionError::savepoint_not_found(id))?;

        if let Some(sync_manager) = self.sync_manager.as_ref() {
            sync_manager
                .rollback_transaction_to_sequence_sync(txn_id, savepoint.sync_sequence)
                .map_err(|e| TransactionError::sync_failed(e.to_string()))?;
        }

        context
            .rollback_to_savepoint(id, target)
            .map_err(|e| TransactionError::rollback_failed(e.to_string()))?;

        Ok(())
    }

    /// Get all active savepoints for transaction
    pub fn get_active_savepoints(&self, txn_id: TransactionId) -> Vec<SavepointInfo> {
        self.get_context(txn_id)
            .map(|ctx| ctx.get_all_savepoints())
            .unwrap_or_default()
    }
}
