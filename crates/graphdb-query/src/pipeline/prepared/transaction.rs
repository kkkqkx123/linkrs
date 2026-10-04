use crate::executor::streaming::transaction_scope::TransactionScope;
use crate::parser::ast::Stmt;
use crate::QueryRequestContext;
use graphdb_core::types::TransactionIsolationLevel;

use super::classify::is_transaction;

/// Resolve the [`TransactionScope`] from a statement and request context.
pub fn resolve_transaction_scope(stmt: &Stmt, request: &QueryRequestContext) -> TransactionScope {
    if is_transaction(stmt) {
        return TransactionScope::CommandScope;
    }
    if let Some(scope) = scope_for_bound_request(request) {
        return scope;
    }
    if let Some(transaction_id) = request.transaction_id {
        if request.auto_commit {
            TransactionScope::auto_commit(transaction_id)
        } else {
            TransactionScope::explicit(transaction_id, !request.read_only)
        }
    } else {
        TransactionScope::None
    }
}

fn scope_for_bound_request(request: &QueryRequestContext) -> Option<TransactionScope> {
    request
        .transaction_id
        .or_else(|| {
            request
                .operation_context
                .as_ref()
                .and_then(|context| context.transaction_id)
        })
        .map(|transaction_id| {
            if request.auto_commit {
                TransactionScope::auto_commit(transaction_id)
            } else {
                TransactionScope::explicit(transaction_id, !request.read_only)
            }
        })
}

/// Derive the MVCC snapshot timestamp for a request.
///
/// Statements inside an explicit transaction inherit the transaction's
/// snapshot timestamp (effective snapshot -> storage operation context read
/// timestamp). Auto-commit statements return `None` so they read the current
/// version of the data.
pub fn snapshot_ts_for_request(
    rctx: &QueryRequestContext,
) -> Option<graphdb_core::types::Timestamp> {
    if rctx.auto_commit {
        return None;
    }
    rctx.operation_context
        .as_ref()
        .map(|context| context.read_timestamp)
}

/// Derive the transaction isolation level for a request.
///
/// The API layer injects the level for queries inside an explicit transaction
/// (from `TransactionExecution`). When it was not injected but the request is
/// still a non-auto-commit transaction statement, fall back to the
/// transaction manager's default (`RepeatableRead`). Auto-commit statements
/// keep `None` (statement-level snapshot semantics).
pub fn isolation_level_for_request(
    rctx: &QueryRequestContext,
) -> Option<TransactionIsolationLevel> {
    if rctx.auto_commit {
        return None;
    }
    rctx.isolation_level.or_else(|| {
        if rctx.transaction_id.is_some() {
            Some(TransactionIsolationLevel::default())
        } else {
            None
        }
    })
}
