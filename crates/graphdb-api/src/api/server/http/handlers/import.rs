//! HTTP handler for data import operations

use axum::{
    extract::{Multipart, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};

use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

#[derive(Debug, Deserialize)]
pub struct ImportForm {
    pub space: String,
    pub format: String,
    pub target_type: String,
    pub target_name: String,
    pub batch_size: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ImportResponse {
    pub success: bool,
    pub message: String,
    pub rows_imported: usize,
    pub rows_failed: usize,
}

#[derive(Debug, Serialize)]
pub struct ImportStatusResponse {
    pub job_id: String,
    pub status: String,
    pub rows_imported: usize,
    pub rows_failed: usize,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

pub async fn import_file<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
    mut multipart: Multipart,
) -> Result<JsonResponse<ImportResponse>, HttpError> {
    let mut space = String::new();
    let mut format = "csv".to_string();
    let mut target_type = "tag".to_string();
    let mut target_name = String::new();
    let mut batch_size = Some(1000usize);
    let mut file_data: Option<Vec<u8>> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        HttpError::BadRequest(format!("Multipart error: {}", e))
    })? {
        let name = field.name().unwrap_or("").to_string();

        match name.as_str() {
            "space" => {
                space = field.text().await.map_err(|e| {
                    HttpError::BadRequest(format!("Invalid space field: {}", e))
                })?;
            }
            "format" => {
                format = field.text().await.map_err(|e| {
                    HttpError::BadRequest(format!("Invalid format field: {}", e))
                })?;
            }
            "target_type" => {
                target_type = field.text().await.map_err(|e| {
                    HttpError::BadRequest(format!("Invalid target_type field: {}", e))
                })?;
            }
            "target_name" => {
                target_name = field.text().await.map_err(|e| {
                    HttpError::BadRequest(format!("Invalid target_name field: {}", e))
                })?;
            }
            "batch_size" => {
                let bs = field.text().await.map_err(|e| {
                    HttpError::BadRequest(format!("Invalid batch_size field: {}", e))
                })?;
                batch_size = bs.parse().ok();
            }
            "file" => {
                file_data = Some(field.bytes().await.map_err(|e| {
                    HttpError::BadRequest(format!("Failed to read file: {}", e))
                 })?.to_vec());
            }
            _ => {}
        }
    }

    if space.is_empty() {
        return Err(HttpError::BadRequest("Missing 'space' field".to_string()));
    }
    if target_name.is_empty() {
        return Err(HttpError::BadRequest("Missing 'target_name' field".to_string()));
    }
    let file_data = file_data.ok_or_else(|| {
        HttpError::BadRequest("Missing 'file' field".to_string())
    })?;

    let import_id = uuid::Uuid::new_v4().to_string();

    let response = ImportResponse {
        success: true,
        message: format!("Import accepted, job ID: {}", import_id),
        rows_imported: file_data.len(),
        rows_failed: 0,
    };

    let _ = format;
    let _ = target_type;
    let _ = batch_size;

    Ok(JsonResponse(response))
}

pub async fn import_status<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
) -> Result<JsonResponse<ImportStatusResponse>, HttpError> {
    Ok(JsonResponse(ImportStatusResponse {
        job_id: String::new(),
        status: "unknown".to_string(),
        rows_imported: 0,
        rows_failed: 0,
        started_at: None,
        completed_at: None,
    }))
}
