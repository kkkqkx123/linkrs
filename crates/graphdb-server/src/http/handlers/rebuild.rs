//! Async index rebuild tasks (fulltext + vector).
//!
//! Rebuilds are long-running: POST handlers accept the request, spawn a
//! background task driving the sync-side online rebuild driver, and return
//! a rebuild id immediately. Callers poll the status endpoint, which merges
//! the stored task record with live rebuild progress while running.
//! Clear endpoints are synchronous and destructive, requiring `force=true`.

#[cfg(any(feature = "fulltext", feature = "vector"))]
use axum::{
    extract::{Path, State},
    response::Json as JsonResponse,
    Json,
};
use serde::Serialize;
use std::sync::Arc;

#[cfg(any(feature = "fulltext", feature = "vector"))]
use crate::http::{error::HttpError, state::AppState};
#[cfg(any(feature = "fulltext", feature = "vector"))]
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

/// One async rebuild task.
#[derive(Debug, Clone, Serialize)]
pub struct RebuildTaskRecord {
    pub rebuild_id: String,
    /// `fulltext` or `vector`.
    pub kind: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    /// `running`, `completed`, or `failed`.
    pub status: String,
    /// Applied point/document operations, set on completion.
    #[serde(default)]
    pub applied: Option<u64>,
    /// Failure reason, set on failure.
    #[serde(default)]
    pub error: Option<String>,
    pub started_at_ms: i64,
}

/// In-memory registry of rebuild tasks, shared via [`crate::http::HttpServer`].
#[derive(Debug, Clone, Default)]
pub struct RebuildTaskRegistry {
    tasks: Arc<dashmap::DashMap<String, RebuildTaskRecord>>,
}

impl RebuildTaskRegistry {
    pub fn register_running(
        &self,
        rebuild_id: String,
        kind: &str,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) {
        self.tasks.insert(
            rebuild_id.clone(),
            RebuildTaskRecord {
                rebuild_id,
                kind: kind.to_string(),
                space_id,
                tag_name: tag_name.to_string(),
                field_name: field_name.to_string(),
                status: "running".to_string(),
                applied: None,
                error: None,
                started_at_ms: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    /// Register a running task unless one is already running for the same
    /// index. Returns false when the request must be rejected with `409
    /// Conflict`: the sync-side driver would return `RebuildBusy` for the
    /// same overlap, so fail fast before spawning the background task.
    pub fn try_register_running(
        &self,
        rebuild_id: String,
        kind: &str,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> bool {
        if self.tasks.iter().any(|entry| {
            entry.value().status == "running"
                && entry.value().kind == kind
                && entry.value().space_id == space_id
                && entry.value().tag_name == tag_name
                && entry.value().field_name == field_name
        }) {
            return false;
        }
        self.register_running(rebuild_id, kind, space_id, tag_name, field_name);
        true
    }

    pub fn finish(&self, rebuild_id: &str, result: Result<u64, String>) {
        if let Some(mut record) = self.tasks.get_mut(rebuild_id) {
            match result {
                Ok(applied) => {
                    record.status = "completed".to_string();
                    record.applied = Some(applied);
                }
                Err(error) => {
                    record.status = "failed".to_string();
                    record.error = Some(error);
                }
            }
        }
    }

    pub fn get(&self, rebuild_id: &str) -> Option<RebuildTaskRecord> {
        self.tasks.get(rebuild_id).map(|entry| entry.clone())
    }
}

#[cfg(any(feature = "fulltext", feature = "vector"))]
fn sync_manager_or_error<S>(
    state: &AppState<S>,
    target: &str,
) -> Result<Arc<graphdb_sync::SyncManager>, HttpError>
where
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + 'static,
{
    state
        .server
        .get_graph_service()
        .sync_api()
        .map(|api| api.sync_manager().clone())
        .ok_or_else(|| {
            HttpError::InternalError(format!(
                "{target} rebuild requires a configured sync manager"
            ))
        })
}

#[cfg(any(feature = "fulltext", feature = "vector"))]
fn resolve_space_name<S>(storage: &S, space_id: u64) -> Result<String, HttpError>
where
    S: StorageClient,
{
    storage
        .get_space_by_id(space_id)
        .map_err(|error| HttpError::InternalError(error.to_string()))?
        .map(|info| info.space_name)
        .ok_or_else(|| HttpError::NotFound(format!("space id {space_id} not found")))
}

// ── Fulltext ──────────────────────────────────────────────────────────────

/// Start an async fulltext index rebuild.
#[cfg(feature = "fulltext")]
pub async fn rebuild_fulltext<
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
    Json(request): Json<graphdb_wire::fulltext::RebuildFulltextIndexRequest>,
) -> Result<JsonResponse<graphdb_wire::fulltext::RebuildFulltextIndexResponse>, HttpError> {
    use graphdb_api::api_core::rebuild_source::StorageRebuildSource;

    let sync_manager = sync_manager_or_error(&state, "fulltext")?;
    let storage = state.server.get_storage().read().clone();
    let space_name = resolve_space_name(&storage, request.space_id)?;
    let rebuild_id = uuid::Uuid::new_v4().to_string();
    let registry = state.server.rebuild_tasks();
    if !registry.try_register_running(
        rebuild_id.clone(),
        "fulltext",
        request.space_id,
        &request.tag_name,
        &request.field_name,
    ) {
        return Err(HttpError::Conflict(format!(
            "fulltext rebuild already running for {}.{}.{}",
            request.space_id, request.tag_name, request.field_name
        )));
    }
    let tasks = registry.clone();
    let task_id = rebuild_id.clone();
    let (space_id, tag_name, field_name) = (
        request.space_id,
        request.tag_name.clone(),
        request.field_name.clone(),
    );
    tokio::spawn(async move {
        let mut source = StorageRebuildSource::new(
            &storage,
            space_name,
            tag_name.clone(),
            field_name.clone(),
            500,
        );
        let result = sync_manager
            .rebuild_fulltext_index(
                space_id,
                &tag_name,
                &field_name,
                &mut source,
                graphdb_sync::FulltextRebuildOptions::default(),
            )
            .await
            .map_err(|error| error.to_string());
        tasks.finish(&task_id, result);
    });
    Ok(JsonResponse(
        graphdb_wire::fulltext::RebuildFulltextIndexResponse {
            rebuild_id,
            status: "running".to_string(),
            space_id: request.space_id,
            tag_name: request.tag_name,
            field_name: request.field_name,
        },
    ))
}

/// Poll a fulltext rebuild task, merging live driver progress while running.
#[cfg(feature = "fulltext")]
pub async fn fulltext_rebuild_status<
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
    Path(rebuild_id): Path<String>,
) -> Result<JsonResponse<graphdb_wire::fulltext::FulltextRebuildStatusResponse>, HttpError> {
    let record = state
        .server
        .rebuild_tasks()
        .get(&rebuild_id)
        .ok_or_else(|| HttpError::NotFound(format!("rebuild task {rebuild_id} not found")))?;
    if record.kind != "fulltext" {
        return Err(HttpError::NotFound(format!(
            "rebuild task {rebuild_id} is not a fulltext rebuild"
        )));
    }
    let mut response = graphdb_wire::fulltext::FulltextRebuildStatusResponse {
        rebuild_id: record.rebuild_id.clone(),
        status: record.status.clone(),
        phase: None,
        generation: None,
        docs_scanned: 0,
        docs_applied: record.applied.unwrap_or(0),
        docs_skipped: 0,
        error: record.error.clone(),
    };
    if record.status == "running" {
        let live = state
            .server
            .get_graph_service()
            .sync_api()
            .and_then(|api| api.sync_manager().fulltext_manager_opt())
            .and_then(|manager| {
                manager.rebuild_progress(record.space_id, &record.tag_name, &record.field_name)
            });
        if let Some(progress) = live {
            response.phase = Some(progress.phase.as_str().to_string());
            response.generation = Some(progress.generation);
            response.docs_scanned = progress.docs_scanned;
            response.docs_applied = progress.docs_applied;
            response.docs_skipped = progress.docs_skipped;
        }
    }
    Ok(JsonResponse(response))
}

/// Synchronously clear a fulltext index. Destructive: requires `force=true`.
#[cfg(feature = "fulltext")]
pub async fn clear_fulltext<
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
    Json(request): Json<graphdb_wire::fulltext::ClearFulltextIndexRequest>,
) -> Result<JsonResponse<graphdb_wire::fulltext::ClearFulltextIndexResponse>, HttpError> {
    if !request.force {
        return Err(HttpError::BadRequest(
            "Refusing to clear fulltext index without explicit confirmation: retry with force = true"
                .to_string(),
        ));
    }
    log::warn!(
        "Clearing fulltext index via HTTP explicitly confirmed: space {} tag {} field {}",
        request.space_id,
        request.tag_name,
        request.field_name
    );
    let sync_manager = sync_manager_or_error(&state, "fulltext")?;
    let manager = sync_manager
        .fulltext_manager_opt()
        .ok_or_else(|| HttpError::InternalError("fulltext target is not configured".to_string()))?;
    manager
        .clear_index(
            request.space_id,
            &request.tag_name,
            &request.field_name,
            true,
        )
        .await
        .map_err(|error| HttpError::InternalError(error.to_string()))?;
    Ok(JsonResponse(
        graphdb_wire::fulltext::ClearFulltextIndexResponse {
            ok: true,
            space_id: request.space_id,
            tag_name: request.tag_name,
            field_name: request.field_name,
        },
    ))
}

/// List fulltext indexes whose engine is in the `Inconsistent` state.
///
/// These indexes reject writes and need operator attention: trigger the
/// async online rebuild (`POST /v1/fulltext/indexes/rebuild`) or drop and
/// recreate the index. Read-only; safe to poll for alerting.
#[cfg(feature = "fulltext")]
pub async fn inconsistent_fulltext<
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
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let sync_manager = sync_manager_or_error(&state, "fulltext")?;
    let inconsistent = sync_manager.inconsistent_fulltext_indexes();
    let count = inconsistent.len() as u64;
    state
        .server
        .get_graph_service()
        .get_stats_manager()
        .record_inconsistent_index_count(count);
    Ok(JsonResponse(serde_json::json!({
        "inconsistent": inconsistent,
        "count": count,
    })))
}

// ── Vector ────────────────────────────────────────────────────────────────

/// Start an async vector index rebuild.
#[cfg(feature = "vector")]
pub async fn rebuild_vector<
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
    Json(request): Json<graphdb_wire::vector::RebuildVectorIndexRequest>,
) -> Result<JsonResponse<graphdb_wire::vector::RebuildVectorIndexResponse>, HttpError> {
    use graphdb_api::api_core::rebuild_source::StorageVectorSource;

    let sync_manager = sync_manager_or_error(&state, "vector")?;
    let storage = state.server.get_storage().read().clone();
    let space_name = resolve_space_name(&storage, request.space_id)?;
    let rebuild_id = uuid::Uuid::new_v4().to_string();
    let registry = state.server.rebuild_tasks();
    if !registry.try_register_running(
        rebuild_id.clone(),
        "vector",
        request.space_id,
        &request.tag_name,
        &request.field_name,
    ) {
        return Err(HttpError::Conflict(format!(
            "vector rebuild already running for {}.{}.{}",
            request.space_id, request.tag_name, request.field_name
        )));
    }
    let tasks = registry.clone();
    let task_id = rebuild_id.clone();
    let (space_id, tag_name, field_name) = (
        request.space_id,
        request.tag_name.clone(),
        request.field_name.clone(),
    );
    tokio::spawn(async move {
        let mut source = StorageVectorSource::new(
            &storage,
            space_name,
            tag_name.clone(),
            field_name.clone(),
            500,
        );
        let result = sync_manager
            .rebuild_vector_index(
                space_id,
                &tag_name,
                &field_name,
                &mut source,
                graphdb_sync::VectorRebuildOptions::default(),
            )
            .await
            .map_err(|error| error.to_string());
        tasks.finish(&task_id, result);
    });
    Ok(JsonResponse(
        graphdb_wire::vector::RebuildVectorIndexResponse {
            rebuild_id,
            status: "running".to_string(),
            space_id: request.space_id,
            tag_name: request.tag_name,
            field_name: request.field_name,
        },
    ))
}

/// Poll a vector rebuild task, merging live driver progress while running.
#[cfg(feature = "vector")]
pub async fn vector_rebuild_status<
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
    Path(rebuild_id): Path<String>,
) -> Result<JsonResponse<graphdb_wire::vector::VectorRebuildStatusResponse>, HttpError> {
    let record = state
        .server
        .rebuild_tasks()
        .get(&rebuild_id)
        .ok_or_else(|| HttpError::NotFound(format!("rebuild task {rebuild_id} not found")))?;
    if record.kind != "vector" {
        return Err(HttpError::NotFound(format!(
            "rebuild task {rebuild_id} is not a vector rebuild"
        )));
    }
    let mut response = graphdb_wire::vector::VectorRebuildStatusResponse {
        rebuild_id: record.rebuild_id.clone(),
        status: record.status.clone(),
        phase: None,
        generation: None,
        vectors_scanned: 0,
        vectors_applied: record.applied.unwrap_or(0),
        vectors_skipped: 0,
        error: record.error.clone(),
    };
    if record.status == "running" {
        let live = state
            .server
            .get_graph_service()
            .sync_api()
            .and_then(|api| api.sync_manager().vector_coordinator().cloned())
            .and_then(|coordinator| {
                coordinator.rebuild_progress(record.space_id, &record.tag_name, &record.field_name)
            });
        if let Some(progress) = live {
            response.phase = Some(progress.phase.as_str().to_string());
            response.generation = Some(progress.generation);
            response.vectors_scanned = progress.docs_scanned;
            response.vectors_applied = progress.docs_applied;
            response.vectors_skipped = progress.docs_skipped;
        }
    }
    Ok(JsonResponse(response))
}

/// Synchronously clear a vector index. Destructive: requires `force=true`.
#[cfg(feature = "vector")]
pub async fn clear_vector<
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
    Json(request): Json<graphdb_wire::vector::ClearVectorIndexRequest>,
) -> Result<JsonResponse<graphdb_wire::vector::ClearVectorIndexResponse>, HttpError> {
    if !request.force {
        return Err(HttpError::BadRequest(
            "Refusing to clear vector index without explicit confirmation: retry with force = true"
                .to_string(),
        ));
    }
    log::warn!(
        "Clearing vector index via HTTP explicitly confirmed: space {} tag {} field {}",
        request.space_id,
        request.tag_name,
        request.field_name
    );
    let sync_manager = sync_manager_or_error(&state, "vector")?;
    let coordinator = sync_manager
        .vector_coordinator()
        .cloned()
        .ok_or_else(|| HttpError::InternalError("vector target is not configured".to_string()))?;
    coordinator
        .index_manager()
        .purge_index_data(request.space_id, &request.tag_name, &request.field_name)
        .await
        .map_err(|error| HttpError::InternalError(error.to_string()))?;
    Ok(JsonResponse(
        graphdb_wire::vector::ClearVectorIndexResponse {
            ok: true,
            space_id: request.space_id,
            tag_name: request.tag_name,
            field_name: request.field_name,
        },
    ))
}
