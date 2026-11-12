//! Monitoring statistics SSE stream.
//!
//! Streams aggregated overview snapshots so the web UI can render live
//! monitoring updates instead of polling. Snapshot collection reuses
//! [`super::overview::collect_overview`] so the streamed payload is
//! identical to `GET /v1/statistics/overview`.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;

use super::overview::collect_overview;
use crate::http::state::AppState;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

const DEFAULT_INTERVAL_MS: u64 = 2_000;
const MIN_INTERVAL_MS: u64 = 500;
const MAX_INTERVAL_MS: u64 = 30_000;

/// Clamp the requested push interval to a sane range.
fn clamp_interval(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(DEFAULT_INTERVAL_MS)
        .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS)
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct StatisticsStreamQuery {
    /// Snapshot push interval in milliseconds.
    pub interval_ms: Option<u64>,
}

#[utoipa::path(
    get,
    operation_id = "get_v1_statistics_stream",
    path = "/v1/statistics/stream",
    tag = "Statistics",
    params(
        ("interval_ms" = Option<u64>, Query, description = "Snapshot push interval in milliseconds (clamped to 500..30000)")
    ),
    responses(
        (status = 200, description = "Monitoring overview snapshot event stream", content_type = "text/event-stream")
    )
)]
/// SSE endpoint streaming aggregated monitoring overview snapshots.
///
/// Each `snapshot` event carries an `OverviewResponse` JSON payload; the
/// stream runs until the client disconnects. A collection failure emits an
/// `error` event and keeps the stream alive so a transient stats hiccup
/// does not tear down live monitoring.
pub async fn statistics_stream<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Query(query): Query<StatisticsStreamQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send + 'static> {
    let interval = clamp_interval(query.interval_ms);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(4);

    // The producer task owns the app state and pushes one snapshot per tick.
    // `interval`'s first tick completes immediately, so clients get an
    // initial snapshot without waiting a full period.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(interval));
        loop {
            ticker.tick().await;
            let event = match collect_overview(&state).await {
                Ok(overview) => serde_json::to_string(&overview)
                    .map(|payload| Event::default().event("snapshot").data(payload))
                    .unwrap_or_else(|e| error_event(&e.to_string())),
                Err(e) => error_event(&e.to_string()),
            };
            if tx.send(Ok(event)).await.is_err() {
                // Receiver dropped: the client disconnected.
                break;
            }
        }
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keepalive"),
    )
}

/// Build the transient-failure event emitted when a snapshot cannot be collected.
fn error_event(message: &str) -> Event {
    let payload = serde_json::json!({
        "code": "STATISTICS_ERROR",
        "message": message,
    });
    Event::default().event("error").data(payload.to_string())
}
