//! HTTP handler for data import operations
//!
//! Parses an uploaded file (CSV with header, JSON array, or JSONL) into DML
//! statements and loads them through the group-commit batch window:
//! each `batch_size` group of statements shares one write timestamp, one WAL
//! fsync, and one commit point, so a large import amortizes fsync across
//! statements instead of syncing per row.
//!
//! File parsing is shared with the embedded API through
//! `linkrs_api::api_core::import`; this handler only owns multipart decoding.

use axum::{
    extract::{Extension, Multipart, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};

use linkrs_api::api_core::import::{parse_import, ImportFormat, ImportTarget};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ImportForm {
    pub space: String,
    pub format: String,
    pub target_type: String,
    pub target_name: String,
    pub batch_size: Option<usize>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ImportResponse {
    pub success: bool,
    pub message: String,
    pub rows_imported: usize,
    pub rows_failed: usize,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ImportStatusResponse {
    pub job_id: String,
    pub status: String,
    pub rows_imported: usize,
    pub rows_failed: usize,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[utoipa::path(
    post,
    path = "/v1/import",
    tag = "Import",
    responses(
        (status = 200, body = ImportResponse, description = "File imported"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn import_file<
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
    Extension(session_id): Extension<i64>,
    mut multipart: Multipart,
) -> Result<JsonResponse<ImportResponse>, HttpError> {
    let mut space = String::new();
    let mut format = "csv".to_string();
    let mut target_type = "tag".to_string();
    let mut target_name = String::new();
    let mut batch_size = Some(1000usize);
    let mut file_data: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| HttpError::BadRequest(format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();

        match name.as_str() {
            "space" => {
                space = field
                    .text()
                    .await
                    .map_err(|e| HttpError::BadRequest(format!("Invalid space field: {e}")))?;
            }
            "format" => {
                format = field
                    .text()
                    .await
                    .map_err(|e| HttpError::BadRequest(format!("Invalid format field: {e}")))?;
            }
            "target_type" => {
                target_type = field.text().await.map_err(|e| {
                    HttpError::BadRequest(format!("Invalid target_type field: {e}"))
                })?;
            }
            "target_name" => {
                target_name = field.text().await.map_err(|e| {
                    HttpError::BadRequest(format!("Invalid target_name field: {e}"))
                })?;
            }
            "batch_size" => {
                let bs = field
                    .text()
                    .await
                    .map_err(|e| HttpError::BadRequest(format!("Invalid batch_size field: {e}")))?;
                batch_size = bs.parse().ok();
            }
            "file" => {
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| HttpError::BadRequest(format!("Failed to read file: {e}")))?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }

    if space.is_empty() {
        return Err(HttpError::BadRequest("Missing 'space' field".to_string()));
    }
    if target_name.is_empty() {
        return Err(HttpError::BadRequest(
            "Missing 'target_name' field".to_string(),
        ));
    }
    let file_data =
        file_data.ok_or_else(|| HttpError::BadRequest("Missing 'file' field".to_string()))?;

    let format = ImportFormat::parse(&format).map_err(|e| HttpError::BadRequest(e.to_string()))?;
    let target =
        ImportTarget::parse(&target_type).map_err(|e| HttpError::BadRequest(e.to_string()))?;
    let parsed = parse_import(format, target, &target_name, &file_data)
        .map_err(|e| HttpError::BadRequest(e.to_string()))?;

    if parsed.is_empty() {
        return Ok(JsonResponse(ImportResponse {
            success: true,
            message: "File contained no importable rows".to_string(),
            rows_imported: 0,
            rows_failed: 0,
        }));
    }

    let statements: Vec<String> = parsed
        .rows
        .iter()
        .map(|row| row.statement.clone())
        .collect();
    let group_size = batch_size.unwrap_or(1000).max(1);
    let outcomes = state
        .server
        .get_graph_service()
        .execute_batch_grouped(session_id, &statements, group_size)
        .await;

    let mut rows_imported = 0;
    let mut rows_failed = parsed.failures.len();
    for outcome in &outcomes {
        match outcome {
            Ok(_) => rows_imported += 1,
            Err(_) => rows_failed += 1,
        }
    }

    let success = rows_failed == 0;
    Ok(JsonResponse(ImportResponse {
        success,
        message: if success {
            format!("Imported {rows_imported} rows into {target_type} '{target_name}'")
        } else {
            format!("Imported {rows_imported} rows, {rows_failed} failed (group size {group_size})")
        },
        rows_imported,
        rows_failed,
    }))
}

#[utoipa::path(
    get,
    path = "/v1/import/{id}",
    tag = "Import",
    params(("id" = String, Path, description = "Import job id")),
    responses(
        (status = 200, body = ImportStatusResponse, description = "Import job status"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn import_status<
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
) -> Result<JsonResponse<ImportStatusResponse>, HttpError> {
    // Imports are executed synchronously; the status endpoint reports the
    // completion contract kept for API compatibility.
    Ok(JsonResponse(ImportStatusResponse {
        job_id: String::new(),
        status: "unknown".to_string(),
        rows_imported: 0,
        rows_failed: 0,
        started_at: None,
        completed_at: None,
    }))
}
