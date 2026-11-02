use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;

/// Cross-subsystem overview (`GET /v1/statistics/overview`).
pub fn overview(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_overview().await
    })
}

/// System resource statistics (`GET /v1/statistics/system`).
pub fn system(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_system_statistics().await
    })
}

/// Query statistics (`GET /v1/statistics/queries`).
pub fn query_stats(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .get_query_statistics()
            .await
            .map(|stats| serde_json::to_value(&stats).unwrap_or(serde_json::Value::Null))
    })
}

/// Database statistics (`GET /v1/statistics/database`).
pub fn database(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .get_database_statistics()
            .await
            .map(|stats| serde_json::to_value(&stats).unwrap_or(serde_json::Value::Null))
    })
}

/// Pull one `f64` series out of a statistics payload by trying a list of
/// candidate JSON paths (`a.b.c`). Missing or non-numeric values read as
/// `None` so charts degrade to gaps instead of failing the whole poll.
pub fn number_at(payload: &serde_json::Value, path: &str) -> Option<f64> {
    let mut current = payload;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    match current {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.parse::<f64>().ok(),
        _ => None,
    }
}
