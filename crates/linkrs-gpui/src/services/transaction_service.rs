use gpui_kit::{App, Task};
use linkrs_client::{ClientError, TransactionOptions};
use linkrs_wire::meta::{SavepointResponse, TransactionResponse};

use crate::connection::SessionStore;
use crate::runtime;

/// Begin a transaction.
pub fn begin(
    cx: &App,
    options: TransactionOptions,
) -> Task<Result<TransactionResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().begin_transaction(options).await
    })
}

/// Commit a transaction.
pub fn commit(cx: &App, txn_id: u64) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().commit_transaction(txn_id).await
    })
}

/// Roll back a transaction.
pub fn rollback(cx: &App, txn_id: u64) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().rollback_transaction(txn_id).await
    })
}

/// List server-side transactions across all sessions.
pub fn list(cx: &App) -> Task<Result<Vec<serde_json::Value>, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().list_transactions().await
    })
}

/// Terminate a server-side transaction.
pub fn kill(cx: &App, txn_id: u64) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().kill_transaction(txn_id).await
    })
}

/// Fetch transaction outcome and resource gauges.
pub fn metrics(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().transaction_metrics().await
    })
}

/// Retry outbox projection scoped to one transaction.
pub fn retry_outbox(
    cx: &App,
    txn_id: u64,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().retry_transaction_outbox(txn_id).await
    })
}

/// Create a savepoint inside an explicit transaction.
pub fn create_savepoint(
    cx: &App,
    txn_id: u64,
    name: Option<String>,
) -> Task<Result<SavepointResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().create_savepoint(txn_id, name).await
    })
}

/// List savepoints of an explicit transaction.
pub fn list_savepoints(
    cx: &App,
    txn_id: u64,
) -> Task<Result<Vec<serde_json::Value>, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().list_savepoints(txn_id).await
    })
}

/// Roll back an explicit transaction to a savepoint.
pub fn rollback_to_savepoint(
    cx: &App,
    txn_id: u64,
    savepoint_id: u64,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .rollback_to_savepoint(txn_id, savepoint_id)
            .await
    })
}

/// Release a savepoint of an explicit transaction.
pub fn release_savepoint(
    cx: &App,
    txn_id: u64,
    savepoint_id: u64,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().release_savepoint(txn_id, savepoint_id).await
    })
}
