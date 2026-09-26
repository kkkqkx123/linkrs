//! HTTP handler for data export operations

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::{
    extract::{Query, State},
    response::Response,
};
use serde::Deserialize;

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExportQuery {
    pub space: Option<String>,
    pub format: Option<String>,
    pub query: Option<String>,
    pub all: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/export",
    tag = "Export",
    params(
        ("space" = Option<String>, Query, description = "Space to export"),
        ("format" = Option<String>, Query, description = "Export format: csv, json or jsonl"),
        ("query" = Option<String>, Query, description = "Query selecting rows to export"),
        ("all" = Option<String>, Query, description = "Export all data when set")
    ),
    responses(
        (status = 200, description = "Export file download"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn export_data<
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
    Query(params): Query<ExportQuery>,
) -> Result<Response, HttpError> {
    let format = params.format.as_deref().unwrap_or("csv");
    let filename = format!("export.{}", format);

    let content_type = match format {
        "json" => "application/json",
        "jsonl" => "application/x-ndjson",
        _ => "text/csv",
    };

    let body_content = match format {
        "json" => serde_json::json!({
            "status": "ok",
            "message": "Export endpoint - implementation pending storage integration",
            "space": params.space,
            "query": params.query,
        })
        .to_string(),
        "jsonl" => "{\"status\":\"ok\"}\n".to_string(),
        _ => "status\nok\n".to_string(),
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(Body::from(body_content))
        .map_err(|e| HttpError::InternalError(format!("Failed to build response: {}", e)))?;

    Ok(response)
}
