//! Migration progress SSE endpoint.
//!
//! The progress hub lives in [`crate::services::migration_progress`]; this
//! module only renders the events as server-sent events.

use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, Sse};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::http::state::AppState;
use crate::services::migration_progress::subscribe;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_migration::MigrationEvent;

fn migration_event_to_sse(event: MigrationEvent) -> Event {
    match event {
        MigrationEvent::Started { plan } => {
            let data = serde_json::json!({
                "type": "started",
                "plan_hash": plan.plan_hash,
                "space": plan.target.space,
                "label": plan.target.label,
                "is_edge": plan.target.is_edge,
            });
            Event::default().event("started").data(data.to_string())
        }
        MigrationEvent::StepStarted { step_idx } => {
            let data = serde_json::json!({
                "type": "step_started",
                "step_idx": step_idx,
            });
            Event::default()
                .event("step_started")
                .data(data.to_string())
        }
        MigrationEvent::StepCompleted { step_idx, rows } => {
            let data = serde_json::json!({
                "type": "step_completed",
                "step_idx": step_idx,
                "rows": rows,
            });
            Event::default()
                .event("step_completed")
                .data(data.to_string())
        }
        MigrationEvent::Completed { report } => {
            let data = serde_json::json!({
                "type": "completed",
                "success": report.success,
                "steps_completed": report.steps_completed,
                "rows_migrated": report.rows_migrated,
                "errors": report.errors,
            });
            Event::default().event("completed").data(data.to_string())
        }
        MigrationEvent::Failed { error } => {
            let data = serde_json::json!({
                "type": "failed",
                "error": error,
            });
            Event::default().event("failed").data(data.to_string())
        }
        MigrationEvent::RolledBack { report } => {
            let data = serde_json::json!({
                "type": "rolled_back",
                "success": report.success,
                "steps_completed": report.steps_completed,
                "rows_migrated": report.rows_migrated,
            });
            Event::default().event("rolled_back").data(data.to_string())
        }
    }
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct MigrationProgressQuery {
    pub is_edge: Option<bool>,
}

#[utoipa::path(
    get,
    path = "/v1/migration/stream/{space}/{label}",
    tag = "Migration",
    params(
        ("space" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name"),
        ("is_edge" = Option<bool>, Query, description = "Whether the label is an edge type")
    ),
    responses(
        (status = 200, description = "Migration progress event stream", content_type = "text/event-stream")
    )
)]
/// SSE endpoint to stream migration progress for a given space/label.
///
/// Example: GET /v1/migration/stream/{space}/{label}?is_edge=false
pub async fn migration_progress_stream<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
    Path((space, label)): Path<(String, String)>,
    Query(query): Query<MigrationProgressQuery>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>> + Send + 'static> {
    let is_edge = query.is_edge.unwrap_or(false);
    let rx = subscribe(&space, &label, is_edge);

    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(ev) => Some(Ok(migration_event_to_sse(ev))),
        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => None,
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(10))
            .text("keepalive"),
    )
}
