use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;

/// Sync/outbox status snapshot.
pub fn status(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().sync_status().await
    })
}

/// Outbox diagnostics snapshot.
pub fn diagnostics(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().outbox_diagnostics().await
    })
}

/// Dead-letter entries with optional target, index and paging filters.
pub fn dead_letters(
    cx: &App,
    target: Option<String>,
    index_id: Option<u64>,
    generation: Option<u64>,
    limit: usize,
    offset: usize,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .outbox_dead_letters(
                target.as_deref(),
                index_id,
                generation,
                limit,
                offset,
            )
            .await
    })
}

/// Requeue dead-letter entries; the request body carries the filter or id set.
pub fn requeue(
    cx: &App,
    request: serde_json::Value,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().outbox_requeue(&request).await
    })
}

/// Retry outbox delivery once.
pub fn retry(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().outbox_retry().await
    })
}

/// Degraded index ranges with optional target, index and generation filters.
pub fn degraded_ranges(
    cx: &App,
    target: Option<String>,
    index_id: Option<u64>,
    generation: Option<u64>,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .outbox_degraded_ranges(target.as_deref(), index_id, generation)
            .await
    })
}

/// Clear one degraded range; the request body carries the range bounds.
pub fn degraded_clear(
    cx: &App,
    request: serde_json::Value,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().outbox_degraded_clear(&request).await
    })
}

/// Outbox retention status snapshot.
pub fn retention_status(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().retention_status().await
    })
}

/// Run outbox retention once with optional distance and age bounds.
pub fn retention_run(
    cx: &App,
    grace_lsn_distance: Option<u64>,
    max_age_ms: Option<u64>,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .retention_run(grace_lsn_distance, max_age_ms)
            .await
    })
}
