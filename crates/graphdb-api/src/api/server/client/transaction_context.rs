use crate::transaction::{SavepointId, TransactionId, TransactionOptions};
use log::info;
use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Debug)]
pub struct TransactionContext {
    current_transaction: Arc<RwLock<Option<TransactionId>>>,
    savepoint_stack: Arc<RwLock<Vec<SavepointId>>>,
    transaction_options: Arc<RwLock<TransactionOptions>>,
    auto_commit: Arc<RwLock<bool>>,
}

impl Default for TransactionContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionContext {
    pub fn new() -> Self {
        Self {
            current_transaction: Arc::new(RwLock::new(None)),
            savepoint_stack: Arc::new(RwLock::new(Vec::new())),
            transaction_options: Arc::new(RwLock::new(TransactionOptions::default())),
            auto_commit: Arc::new(RwLock::new(true)),
        }
    }

    pub fn current_transaction(&self) -> Option<TransactionId> {
        *self.current_transaction.read()
    }

    pub fn bind_transaction(&self, txn_id: TransactionId, session_id: i64) {
        info!("Binding transaction {} to session {}", txn_id, session_id);
        *self.current_transaction.write() = Some(txn_id);
    }

    pub fn unbind_transaction(&self, session_id: i64) {
        if let Some(txn_id) = self.current_transaction() {
            info!(
                "Unbinding transaction {} from session {}",
                txn_id, session_id
            );
            *self.current_transaction.write() = None;
            self.savepoint_stack.write().clear();
        }
    }

    pub fn has_active_transaction(&self) -> bool {
        self.current_transaction().is_some()
    }

    pub fn is_auto_commit(&self) -> bool {
        *self.auto_commit.read()
    }

    pub fn set_auto_commit(&self, auto_commit: bool, session_id: i64) {
        info!(
            "Setting auto_commit to {} for session {}",
            auto_commit, session_id
        );
        *self.auto_commit.write() = auto_commit;
    }

    pub fn transaction_options(&self) -> TransactionOptions {
        self.transaction_options.read().clone()
    }

    pub fn set_transaction_options(&self, options: TransactionOptions) {
        *self.transaction_options.write() = options;
    }

    pub fn push_savepoint(&self, savepoint_id: SavepointId, session_id: i64) {
        info!(
            "Pushing savepoint {} to session {}",
            savepoint_id, session_id
        );
        self.savepoint_stack.write().push(savepoint_id);
    }

    pub fn savepoint_stack(&self) -> Vec<SavepointId> {
        self.savepoint_stack.read().clone()
    }

    pub fn clear_savepoints(&self, session_id: i64) {
        info!("Clearing savepoint stack for session {}", session_id);
        self.savepoint_stack.write().clear();
    }

    pub fn savepoint_count(&self) -> usize {
        self.savepoint_stack.read().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_context() {
        let context = TransactionContext::new();
        assert!(context.current_transaction().is_none());
        assert!(!context.has_active_transaction());
        assert!(context.is_auto_commit());

        context.bind_transaction(TransactionId::from(1001u64), 123);
        assert_eq!(
            context.current_transaction(),
            Some(TransactionId::from(1001u64))
        );
        assert!(context.has_active_transaction());

        context.unbind_transaction(123);
        assert!(context.current_transaction().is_none());
    }

    #[test]
    fn test_auto_commit() {
        let context = TransactionContext::new();
        assert!(context.is_auto_commit());

        context.set_auto_commit(false, 123);
        assert!(!context.is_auto_commit());
    }

    #[test]
    fn test_savepoint_stack() {
        let context = TransactionContext::new();
        assert_eq!(context.savepoint_count(), 0);
        assert!(context.savepoint_stack().is_empty());

        context.push_savepoint(1, 123);
        context.push_savepoint(2, 123);
        assert_eq!(context.savepoint_count(), 2);
        assert_eq!(context.savepoint_stack(), vec![1, 2]);

        context.clear_savepoints(123);
        assert_eq!(context.savepoint_count(), 0);
    }
}
