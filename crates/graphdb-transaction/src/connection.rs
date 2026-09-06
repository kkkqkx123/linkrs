//! Per-Connection Transaction Context
//!
//! Provides connection-level transaction management mirroring Ladybug's
//! `TransactionContext` hierarchy (Connection -> Transaction -> Manager).
//! Each client connection owns a `ConnectionContext` that tracks the
//! current transaction and whether the connection operates in AUTO or MANUAL mode.
//!
//! In AUTO mode every statement auto-commits (single-statement transaction).
//! In MANUAL mode the client must explicitly BEGIN/COMMIT.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::RwLock;

use crate::error::TransactionError;
use crate::manager::TransactionManager;
use crate::types::{SavepointId, TransactionId, TransactionOptions};

/// Unique connection identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(pub u64);

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "conn-{}", self.0)
    }
}

impl From<u64> for ConnectionId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<ConnectionId> for u64 {
    fn from(c: ConnectionId) -> Self {
        c.0
    }
}

/// Transaction mode for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransactionMode {
    /// Each statement runs in its own auto-committed transaction.
    #[default]
    AutoCommit,
    /// Client must explicitly BEGIN and COMMIT/ROLLBACK.
    Manual,
}

impl std::fmt::Display for TransactionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionMode::AutoCommit => write!(f, "AUTO"),
            TransactionMode::Manual => write!(f, "MANUAL"),
        }
    }
}

/// Per-connection transaction context.
///
/// This is the connection-level analogue of Ladybug's `TransactionContext`.
/// It sits above `TransactionContext` (per-transaction) and is owned by the
/// client/session layer. The `TransactionManager` remains the global coordinator.
#[derive(Debug)]
pub struct ConnectionContext {
    id: ConnectionId,
    mode: RwLock<TransactionMode>,
    current_transaction: RwLock<Option<TransactionId>>,
    savepoint_stack: RwLock<Vec<SavepointId>>,
    transaction_options: RwLock<TransactionOptions>,
    auto_commit: RwLock<bool>,
    created_at: Instant,
}

impl ConnectionContext {
    /// Create a new connection context in AUTO mode.
    pub fn new(id: ConnectionId) -> Self {
        Self {
            id,
            mode: RwLock::new(TransactionMode::AutoCommit),
            current_transaction: RwLock::new(None),
            savepoint_stack: RwLock::new(Vec::new()),
            transaction_options: RwLock::new(TransactionOptions::default()),
            auto_commit: RwLock::new(true),
            created_at: Instant::now(),
        }
    }

    /// Create with explicit mode.
    pub fn with_mode(id: ConnectionId, mode: TransactionMode) -> Self {
        let auto_commit = mode == TransactionMode::AutoCommit;
        Self {
            id,
            mode: RwLock::new(mode),
            current_transaction: RwLock::new(None),
            savepoint_stack: RwLock::new(Vec::new()),
            transaction_options: RwLock::new(TransactionOptions::default()),
            auto_commit: RwLock::new(auto_commit),
            created_at: Instant::now(),
        }
    }

    pub fn id(&self) -> ConnectionId {
        self.id
    }

    pub fn mode(&self) -> TransactionMode {
        *self.mode.read()
    }

    pub fn set_mode(&self, mode: TransactionMode) {
        *self.mode.write() = mode;
        *self.auto_commit.write() = mode == TransactionMode::AutoCommit;
    }

    pub fn current_transaction(&self) -> Option<TransactionId> {
        *self.current_transaction.read()
    }

    pub fn has_active_transaction(&self) -> bool {
        self.current_transaction().is_some()
    }

    pub fn bind_transaction(&self, txn_id: TransactionId) {
        *self.current_transaction.write() = Some(txn_id);
    }

    pub fn unbind_transaction(&self) {
        *self.current_transaction.write() = None;
        self.savepoint_stack.write().clear();
    }

    pub fn is_auto_commit(&self) -> bool {
        *self.auto_commit.read()
    }

    pub fn set_auto_commit(&self, auto_commit: bool) {
        *self.auto_commit.write() = auto_commit;
        let mode = if auto_commit {
            TransactionMode::AutoCommit
        } else {
            TransactionMode::Manual
        };
        *self.mode.write() = mode;
    }

    pub fn transaction_options(&self) -> TransactionOptions {
        self.transaction_options.read().clone()
    }

    pub fn set_transaction_options(&self, options: TransactionOptions) {
        *self.transaction_options.write() = options;
    }

    pub fn push_savepoint(&self, savepoint_id: SavepointId) {
        self.savepoint_stack.write().push(savepoint_id);
    }

    pub fn savepoint_stack(&self) -> Vec<SavepointId> {
        self.savepoint_stack.read().clone()
    }

    pub fn clear_savepoints(&self) {
        self.savepoint_stack.write().clear();
    }

    pub fn savepoint_count(&self) -> usize {
        self.savepoint_stack.read().len()
    }

    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }
}

/// Manager for per-connection contexts.
///
/// Provides connection lifecycle and AUTO/MANUAL execution helpers.
/// This reduces `TransactionManager` contention by grouping transactions
/// under their owning connection.
pub struct ConnectionManager {
    connections: dashmap::DashMap<ConnectionId, std::sync::Arc<ConnectionContext>>,
    id_generator: AtomicU64,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: dashmap::DashMap::new(),
            id_generator: AtomicU64::new(1),
        }
    }

    /// Create a new connection in AUTO mode and return its id.
    pub fn create_connection(&self) -> ConnectionId {
        self.create_connection_with_mode(TransactionMode::AutoCommit)
    }

    pub fn create_connection_with_mode(&self, mode: TransactionMode) -> ConnectionId {
        let id = ConnectionId(self.id_generator.fetch_add(1, Ordering::SeqCst));
        let ctx = std::sync::Arc::new(ConnectionContext::with_mode(id, mode));
        self.connections.insert(id, ctx);
        id
    }

    pub fn get_connection(&self, id: ConnectionId) -> Option<std::sync::Arc<ConnectionContext>> {
        self.connections.get(&id).map(|e| e.value().clone())
    }

    pub fn remove_connection(&self, id: ConnectionId) -> Option<std::sync::Arc<ConnectionContext>> {
        self.connections.remove(&id).map(|(_, v)| v)
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Canonical owner string for transactions bound to a connection.
    ///
    /// The transaction-level `owner` field carries this projection of the
    /// `ConnectionId`, so manager-level ownership checks
    /// (`check_transaction_owner`) and connection binding share one source
    /// instead of two parallel mechanisms. The `conn:` prefix namespaces
    /// connection owners away from session-id owners used by the HTTP/gRPC
    /// APIs (`begin_transaction_with_owner`), so a session id that happens
    /// to equal a connection id can never claim another connection's
    /// transaction.
    pub fn owner_for_connection(conn_id: ConnectionId) -> String {
        format!("conn:{conn_id}")
    }

    /// Begin a transaction for the given connection.
    ///
    /// In AUTO mode this is used internally by `execute_auto_commit`.
    /// In MANUAL mode the caller must explicitly commit/abort.
    ///
    /// The transaction is stamped with this connection as its owner, so a
    /// different connection cannot commit or abort it through the
    /// owner-checked manager APIs.
    pub fn begin_for_connection(
        &self,
        manager: &TransactionManager,
        conn_id: ConnectionId,
        options: TransactionOptions,
    ) -> Result<TransactionId, TransactionError> {
        let conn = self.get_connection(conn_id).ok_or_else(|| {
            TransactionError::internal(format!("Connection {} not found", conn_id))
        })?;
        if conn.has_active_transaction() {
            return Err(TransactionError::internal(format!(
                "Connection {} already has active transaction {:?}",
                conn_id,
                conn.current_transaction()
            )));
        }
        let txn_id = manager.begin_transaction(options)?;
        if let Err(error) =
            manager.set_transaction_owner(txn_id, Self::owner_for_connection(conn_id))
        {
            let _ = manager.abort_transaction(txn_id);
            return Err(error);
        }
        conn.bind_transaction(txn_id);
        Ok(txn_id)
    }

    /// Commit the connection's current transaction.
    ///
    /// Ownership is verified before committing: a transaction bound to
    /// another connection is rejected with `transaction_not_owner`.
    pub fn commit_for_connection(
        &self,
        manager: &TransactionManager,
        conn_id: ConnectionId,
    ) -> Result<(), TransactionError> {
        let conn = self.get_connection(conn_id).ok_or_else(|| {
            TransactionError::internal(format!("Connection {} not found", conn_id))
        })?;
        let txn_id = conn.current_transaction().ok_or_else(|| {
            TransactionError::internal(format!("Connection {} has no active transaction", conn_id))
        })?;
        manager.check_transaction_owner(txn_id, Some(&Self::owner_for_connection(conn_id)))?;
        manager.commit_transaction(txn_id)?;
        conn.unbind_transaction();
        Ok(())
    }

    /// Abort the connection's current transaction.
    ///
    /// Ownership is verified before aborting: a transaction bound to
    /// another connection is rejected with `transaction_not_owner`.
    pub fn abort_for_connection(
        &self,
        manager: &TransactionManager,
        conn_id: ConnectionId,
    ) -> Result<(), TransactionError> {
        let conn = self.get_connection(conn_id).ok_or_else(|| {
            TransactionError::internal(format!("Connection {} not found", conn_id))
        })?;
        let txn_id = conn.current_transaction().ok_or_else(|| {
            TransactionError::internal(format!("Connection {} has no active transaction", conn_id))
        })?;
        manager.check_transaction_owner(txn_id, Some(&Self::owner_for_connection(conn_id)))?;
        manager.abort_transaction(txn_id)?;
        conn.unbind_transaction();
        Ok(())
    }

    /// Execute a single statement with AUTO-commit semantics for the connection.
    ///
    /// If the connection is in AUTO mode, a transaction is begun, the closure
    /// is executed, and the transaction is committed (or aborted on error).
    /// If in MANUAL mode and a transaction is already active, the closure runs
    /// inside that transaction without auto-commit. A MANUAL connection
    /// without an active transaction is an explicit error instead of an
    /// implicit one-shot transaction.
    pub fn execute_auto_commit<F, T, E>(
        &self,
        manager: &TransactionManager,
        conn_id: ConnectionId,
        options: TransactionOptions,
        operation: F,
    ) -> Result<T, TransactionError>
    where
        F: FnOnce(&crate::context::TransactionContext) -> Result<T, E>,
        E: Into<TransactionError>,
    {
        let conn = self.get_connection(conn_id).ok_or_else(|| {
            TransactionError::internal(format!("Connection {} not found", conn_id))
        })?;

        // If MANUAL with active txn, run inside existing transaction
        if conn.mode() == TransactionMode::Manual {
            if let Some(txn_id) = conn.current_transaction() {
                let ctx = manager.get_context(txn_id)?;
                return operation(&ctx).map_err(Into::into);
            }
            return Err(TransactionError::internal(format!(
                "Connection {} in MANUAL mode has no active transaction",
                conn_id
            )));
        }

        // AUTO mode: one-shot transaction (MANUAL without an active
        // transaction was rejected above).
        let txn_id = manager.begin_transaction(options)?;
        // Only bind if MANUAL mode expects to keep it (but for auto-commit helper we commit immediately,
        // so we don't bind to connection; we just use the txn directly)
        let ctx = manager.get_context(txn_id)?;
        match operation(&ctx) {
            Ok(result) => {
                manager.commit_transaction(txn_id)?;
                Ok(result)
            }
            Err(e) => {
                let _ = manager.abort_transaction(txn_id);
                Err(e.into())
            }
        }
    }

    /// Execute with connection's existing transaction or create auto-commit.
    pub fn with_connection_transaction<F, T, E>(
        &self,
        manager: &TransactionManager,
        conn_id: ConnectionId,
        operation: F,
    ) -> Result<T, TransactionError>
    where
        F: FnOnce(&crate::context::TransactionContext) -> Result<T, E>,
        E: Into<TransactionError>,
    {
        let conn = self.get_connection(conn_id).ok_or_else(|| {
            TransactionError::internal(format!("Connection {} not found", conn_id))
        })?;

        if let Some(txn_id) = conn.current_transaction() {
            let ctx = manager.get_context(txn_id)?;
            return operation(&ctx).map_err(Into::into);
        }

        if conn.is_auto_commit() {
            self.execute_auto_commit(manager, conn_id, conn.transaction_options(), operation)
        } else {
            Err(TransactionError::internal(format!(
                "Connection {} in MANUAL mode has no active transaction",
                conn_id
            )))
        }
    }
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TransactionManager, TransactionManagerConfig};

    #[test]
    fn test_connection_context_basic() {
        let ctx = ConnectionContext::new(ConnectionId(1));
        assert_eq!(ctx.id(), ConnectionId(1));
        assert_eq!(ctx.mode(), TransactionMode::AutoCommit);
        assert!(ctx.is_auto_commit());
        assert!(!ctx.has_active_transaction());
        assert_eq!(ctx.current_transaction(), None);
    }

    #[test]
    fn test_connection_context_mode_switch() {
        let ctx = ConnectionContext::new(ConnectionId(1));
        ctx.set_mode(TransactionMode::Manual);
        assert_eq!(ctx.mode(), TransactionMode::Manual);
        assert!(!ctx.is_auto_commit());

        ctx.set_auto_commit(true);
        assert_eq!(ctx.mode(), TransactionMode::AutoCommit);
        assert!(ctx.is_auto_commit());
    }

    #[test]
    fn test_connection_context_bind_unbind() {
        let ctx = ConnectionContext::new(ConnectionId(1));
        let txn_id = TransactionId(42);
        ctx.bind_transaction(txn_id);
        assert_eq!(ctx.current_transaction(), Some(txn_id));
        assert!(ctx.has_active_transaction());

        ctx.push_savepoint(10);
        ctx.push_savepoint(20);
        assert_eq!(ctx.savepoint_count(), 2);

        ctx.unbind_transaction();
        assert_eq!(ctx.current_transaction(), None);
        assert_eq!(ctx.savepoint_count(), 0);
    }

    #[test]
    fn test_connection_manager_create_and_remove() {
        let mgr = ConnectionManager::new();
        let id1 = mgr.create_connection();
        let id2 = mgr.create_connection_with_mode(TransactionMode::Manual);
        assert_ne!(id1, id2);
        assert_eq!(mgr.connection_count(), 2);

        let ctx = mgr.get_connection(id1).unwrap();
        assert_eq!(ctx.id(), id1);

        mgr.remove_connection(id1);
        assert_eq!(mgr.connection_count(), 1);
        assert!(mgr.get_connection(id1).is_none());
    }

    #[test]
    fn test_connection_manager_begin_commit() {
        let txn_mgr = TransactionManager::new(TransactionManagerConfig::default());
        let conn_mgr = ConnectionManager::new();
        let conn_id = conn_mgr.create_connection_with_mode(TransactionMode::Manual);

        let txn_id = conn_mgr
            .begin_for_connection(&txn_mgr, conn_id, crate::TransactionOptions::default())
            .expect("begin should succeed");
        assert_eq!(
            conn_mgr
                .get_connection(conn_id)
                .unwrap()
                .current_transaction(),
            Some(txn_id)
        );

        conn_mgr
            .commit_for_connection(&txn_mgr, conn_id)
            .expect("commit should succeed");
        assert_eq!(
            conn_mgr
                .get_connection(conn_id)
                .unwrap()
                .current_transaction(),
            None
        );
    }

    #[test]
    fn test_connection_manager_auto_commit_execution() {
        let txn_mgr = TransactionManager::new(TransactionManagerConfig::default());
        let conn_mgr = ConnectionManager::new();
        let conn_id = conn_mgr.create_connection();

        let result: Result<u64, TransactionError> = conn_mgr.execute_auto_commit(
            &txn_mgr,
            conn_id,
            crate::TransactionOptions::default(),
            |ctx| Ok::<u64, TransactionError>(ctx.id.0),
        );
        assert!(result.is_ok());
        assert!(result.unwrap() > 0);
        // AUTO execution should not leave transaction bound
        assert_eq!(
            conn_mgr
                .get_connection(conn_id)
                .unwrap()
                .current_transaction(),
            None
        );
    }

    #[test]
    fn test_read_only_transaction_rejects_journal_writes() {
        use crate::{TransactionErrorKind, TransactionManagerConfig};

        let txn_mgr = TransactionManager::new(TransactionManagerConfig::default());
        let reader = txn_mgr
            .begin_read_transaction(crate::TransactionOptions::default().read_only())
            .expect("reader should begin");

        // Statement entry is intent-free and succeeds for reads.
        let (context, start) = txn_mgr
            .begin_statement(reader)
            .expect("begin should succeed");
        txn_mgr
            .finish_statement(&context, start)
            .expect("finish should succeed");
        txn_mgr
            .refresh_statement_snapshot(reader)
            .expect("refresh should succeed");

        // But the journal itself rejects writes on read-only transactions, so
        // a write that slips past the query-layer plan check fails fast here
        // instead of being silently dropped at commit.
        let context = txn_mgr.get_context(reader).expect("context should exist");
        let error = context
            .record_mutation(crate::types::MutationResult::new(
                crate::types::MutationEntityKey::Vertex(crate::VertexId::from_int64(1)),
            ))
            .expect_err("journal write on a read-only transaction must be rejected");
        assert_eq!(error.kind(), TransactionErrorKind::ReadOnlyTransaction);
    }

    #[test]
    fn test_connection_ownership_is_enforced_across_connections() {
        use crate::{TransactionErrorKind, TransactionManagerConfig};

        let txn_mgr = TransactionManager::new(TransactionManagerConfig::default());
        let conn_mgr = ConnectionManager::new();
        let conn_a = conn_mgr.create_connection_with_mode(TransactionMode::Manual);
        let conn_b = conn_mgr.create_connection_with_mode(TransactionMode::Manual);

        let txn_id = conn_mgr
            .begin_for_connection(&txn_mgr, conn_a, crate::TransactionOptions::default())
            .expect("begin should succeed");

        // The transaction carries connection A's ownership.
        let context = txn_mgr.get_context(txn_id).expect("context should exist");
        assert_eq!(
            context.owner().as_deref(),
            Some(ConnectionManager::owner_for_connection(conn_a).as_str())
        );

        // Connection B cannot take over through the owner-checked manager API.
        let error = txn_mgr
            .commit_transaction_as_owner(
                txn_id,
                Some(&ConnectionManager::owner_for_connection(conn_b)),
            )
            .expect_err("cross-connection commit must be rejected");
        assert_eq!(error.kind(), TransactionErrorKind::TransactionNotOwner);

        // Connection B has no transaction of its own either.
        assert!(conn_mgr.commit_for_connection(&txn_mgr, conn_b).is_err());

        // The owning connection commits normally.
        conn_mgr
            .commit_for_connection(&txn_mgr, conn_a)
            .expect("owner commit should succeed");
    }

    #[test]
    fn test_execute_auto_commit_in_manual_without_txn_fails() {
        use crate::{TransactionError, TransactionManagerConfig};

        let txn_mgr = TransactionManager::new(TransactionManagerConfig::default());
        let conn_mgr = ConnectionManager::new();
        let conn_id = conn_mgr.create_connection_with_mode(TransactionMode::Manual);

        let result: Result<(), TransactionError> = conn_mgr.execute_auto_commit(
            &txn_mgr,
            conn_id,
            crate::TransactionOptions::default(),
            |_| Ok::<(), TransactionError>(()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_manager_manual_without_txn_fails() {
        let txn_mgr = TransactionManager::new(TransactionManagerConfig::default());
        let conn_mgr = ConnectionManager::new();
        let conn_id = conn_mgr.create_connection_with_mode(TransactionMode::Manual);

        let result: Result<(), TransactionError> =
            conn_mgr
                .with_connection_transaction(&txn_mgr, conn_id, |_| Ok::<(), TransactionError>(()));
        assert!(result.is_err());
    }
}
