use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, RwLock};

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, Sse};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::http::state::AppState;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use graphdb_migration::{MigrationEvent, MigrationEventListener};

type Sender = tokio::sync::broadcast::Sender<MigrationEvent>;
type Receiver = tokio::sync::broadcast::Receiver<MigrationEvent>;

static HUB: std::sync::OnceLock<Arc<RwLock<HashMap<String, Sender>>>> = std::sync::OnceLock::new();

fn hub() -> Arc<RwLock<HashMap<String, Sender>>> {
    HUB.get_or_init(|| Arc::new(RwLock::new(HashMap::new()))).clone()
}

fn hub_key(space: &str, label: &str, is_edge: bool) -> String {
    format!("{}/{}/{}", space, label, is_edge)
}

pub fn get_or_create_sender(space: &str, label: &str, is_edge: bool) -> Sender {
    let key = hub_key(space, label, is_edge);
    let hub_arc = hub();
    {
        let read = hub_arc.read().unwrap();
        if let Some(s) = read.get(&key) {
            return s.clone();
        }
    }
    let mut write = hub_arc.write().unwrap();
    if let Some(s) = write.get(&key) {
        return s.clone();
    }
    let (tx, _rx) = tokio::sync::broadcast::channel(100);
    write.insert(key, tx.clone());
    tx
}

pub fn subscribe(space: &str, label: &str, is_edge: bool) -> Receiver {
    get_or_create_sender(space, label, is_edge).subscribe()
}

pub struct BroadcastEventListener {
    sender: Sender,
}

impl BroadcastEventListener {
    pub fn new(space: &str, label: &str, is_edge: bool) -> Self {
        Self {
            sender: get_or_create_sender(space, label, is_edge),
        }
    }

    pub fn from_sender(sender: Sender) -> Self {
        Self { sender }
    }
}

impl MigrationEventListener for BroadcastEventListener {
    fn on_event(&self, event: MigrationEvent) {
        let _ = self.sender.send(event);
    }
}

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
            Event::default()
                .event("started")
                .data(data.to_string())
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
            Event::default()
                .event("completed")
                .data(data.to_string())
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
            Event::default()
                .event("rolled_back")
                .data(data.to_string())
        }
    }
}

#[derive(serde::Deserialize)]
pub struct MigrationProgressQuery {
    pub is_edge: Option<bool>,
}

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
