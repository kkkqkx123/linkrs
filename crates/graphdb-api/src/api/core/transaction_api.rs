//! Transaction Management API - Core Layer
//!
//! Provides transport layer-independent transaction management capabilities

use crate::api::core::{CoreError, CoreResult, SavepointId, TransactionHandle};
use crate::transaction::{TransactionManager, TransactionOptions};
use std::sync::Arc;

/// Common Transaction API - Core Layer
pub struct TransactionApi {
    txn_manager: Arc<TransactionManager>,
}

impl TransactionApi {
    /// Creating a New Transaction API Instance
    pub fn new(txn_manager: Arc<TransactionManager>) -> Self {
        Self { txn_manager }
    }

    /// Commencement of business
    ///
    /// # Parameters
    /// - `options`: transaction options
    ///
    /// # Back
    /// transaction handle
    pub fn begin(&self, options: TransactionOptions) -> CoreResult<TransactionHandle> {
        let txn_id = self
            .txn_manager
            .begin_transaction(options)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        Ok(TransactionHandle(txn_id))
    }

    /// Submission of transactions
    ///
    /// # Parameters
    /// - `handle`: transaction handle
    pub fn commit(&self, handle: TransactionHandle) -> CoreResult<()> {
        self.txn_manager
            .commit_transaction(handle.0)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Rolling back (aborting) transactions
    ///
    /// # Parameters
    /// `handle`: Transaction handler
    pub fn rollback(&self, handle: TransactionHandle) -> CoreResult<()> {
        self.txn_manager
            .abort_transaction(handle.0)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Getting Transaction Status
    ///
    /// # Parameters
    /// - `handle`: transaction handle
    ///
    /// # Return
    /// Transaction Status String
    pub fn get_status(&self, handle: TransactionHandle) -> CoreResult<String> {
        match self.txn_manager.get_transaction_info(handle.0) {
            Some(info) => Ok(format!("{:?}", info.state)),
            None => Ok("Unknown".to_string()),
        }
    }

    /// Check if a transaction exists and is active
    ///
    /// # Parameters
    /// - `handle`: transaction handle
    pub fn is_active(&self, handle: TransactionHandle) -> bool {
        self.txn_manager.is_transaction_active(handle.0)
    }

    /// Get the number of active transactions
    pub fn active_count(&self) -> usize {
        self.txn_manager.list_active_transactions().len()
    }

    /// Create a savepoint within a transaction
    ///
    /// # Parameters
    /// - `handle`: transaction handle
    /// - `name`: optional savepoint name
    ///
    /// # Returns
    /// Savepoint ID on success
    pub fn create_savepoint(
        &self,
        handle: TransactionHandle,
        name: Option<String>,
    ) -> CoreResult<SavepointId> {
        self.txn_manager
            .create_savepoint(handle.0, name)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
            .map(SavepointId)
    }

    /// Rollback to a savepoint
    ///
    /// # Parameters
    /// - `handle`: transaction handle
    /// - `savepoint_id`: savepoint ID to rollback to
    /// - `storage`: reference to storage for undo operations
    pub fn rollback_to_savepoint(
        &self,
        handle: TransactionHandle,
        savepoint_id: SavepointId,
        storage: &impl graphdb_storage::storage::UndoTarget,
    ) -> CoreResult<()> {
        self.txn_manager
            .rollback_to_savepoint(handle.0, savepoint_id.0, storage)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Release a savepoint
    ///
    /// # Parameters
    /// - `handle`: transaction handle
    /// - `savepoint_id`: savepoint ID to release
    pub fn release_savepoint(
        &self,
        handle: TransactionHandle,
        savepoint_id: SavepointId,
    ) -> CoreResult<()> {
        let context = self
            .txn_manager
            .get_context(handle.0)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        context
            .release_savepoint(savepoint_id.0)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        Ok(())
    }

    /// Get all savepoints for a transaction
    ///
    /// # Parameters
    /// - `handle`: transaction handle
    pub fn get_savepoints(
        &self,
        handle: TransactionHandle,
    ) -> CoreResult<Vec<crate::transaction::SavepointInfo>> {
        let context = self
            .txn_manager
            .get_context(handle.0)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        Ok(context.get_all_savepoints())
    }
}

impl Clone for TransactionApi {
    fn clone(&self) -> Self {
        Self {
            txn_manager: Arc::clone(&self.txn_manager),
        }
    }
}
