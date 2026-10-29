use gpui_kit::{App, Task};
use linkrs_client::{ClientError, TransactionOptions};
use linkrs_wire::meta::TransactionResponse;

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
