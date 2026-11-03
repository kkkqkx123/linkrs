//! HTTP handler for data import operations
//!
//! Parses an uploaded file (CSV with header, JSON array, or JSONL) directly
//! into typed batch items and loads them through the core bulk insert path,
//! bypassing the statement pipeline: no per-row parsing, planning, or
//! optimization, and native JSON numbers and booleans keep their types
//! instead of degrading to strings.

use axum::{
    extract::{Extension, Multipart, Path, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::batch::{BatchItem, EdgeData, VertexData};
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

/// Maximum import job records retained for status polling.
const MAX_IMPORT_JOBS: usize = 512;

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
    pub job_id: String,
    pub message: String,
    pub rows_imported: usize,
    pub rows_failed: usize,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ImportJobRecord {
    pub job_id: String,
    pub space: String,
    pub format: String,
    pub target_type: String,
    pub target_name: String,
    /// `completed` or `failed`.
    pub status: String,
    pub rows_imported: usize,
    pub rows_failed: usize,
    pub started_at: String,
    pub completed_at: String,
}

/// In-memory registry of finished import jobs, shared through
/// [`crate::http::HttpServer`].
#[derive(Debug, Clone, Default)]
pub struct ImportJobRegistry {
    jobs: Arc<dashmap::DashMap<String, ImportJobRecord>>,
}

impl ImportJobRegistry {
    pub fn record(&self, record: ImportJobRecord) {
        self.jobs.insert(record.job_id.clone(), record);
        if self.jobs.len() > MAX_IMPORT_JOBS {
            let oldest = self
                .jobs
                .iter()
                .min_by_key(|entry| entry.value().started_at.clone())
                .map(|entry| entry.key().clone());
            if let Some(id) = oldest {
                self.jobs.remove(&id);
            }
        }
    }

    pub fn get(&self, job_id: &str) -> Option<ImportJobRecord> {
        self.jobs.get(job_id).map(|entry| entry.clone())
    }
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

/// Infer a JSON value from a CSV cell: integers and floats keep numeric
/// types, recognized literals become booleans or null, everything else
/// stays a string. Non-finite floats fall back to strings because JSON
/// has no representation for them.
fn parse_cell(raw: &str) -> serde_json::Value {
    if raw.is_empty() {
        return serde_json::Value::Null;
    }
    if let Ok(int) = raw.parse::<i64>() {
        return serde_json::Value::from(int);
    }
    if let Ok(float) = raw.parse::<f64>() {
        if let Some(number) = serde_json::Number::from_f64(float) {
            return serde_json::Value::Number(number);
        }
    }
    if raw.eq_ignore_ascii_case("true") {
        return serde_json::Value::Bool(true);
    }
    if raw.eq_ignore_ascii_case("false") {
        return serde_json::Value::Bool(false);
    }
    serde_json::Value::String(raw.to_string())
}

/// Build one vertex batch item from a CSV record. Column 0 is the VID and
/// stays a raw string; remaining columns infer their types.
fn vertex_item_from_csv(
    target_name: &str,
    headers: &csv::StringRecord,
    record: &csv::StringRecord,
) -> Result<BatchItem, HttpError> {
    let vid = record
        .get(0)
        .ok_or_else(|| HttpError::BadRequest("CSV vertex rows must start with the VID".into()))?;
    let mut properties = HashMap::new();
    for (index, name) in headers.iter().enumerate() {
        let raw = record.get(index).unwrap_or("");
        let value = if index == 0 {
            serde_json::Value::String(raw.to_string())
        } else {
            parse_cell(raw)
        };
        properties.insert(name.to_string(), value);
    }
    Ok(BatchItem::Vertex(VertexData {
        vid: serde_json::Value::String(vid.to_string()),
        tag: target_name.to_string(),
        properties,
    }))
}

/// Build one edge batch item from a CSV record. Columns 0/1 are the source
/// and destination VIDs and stay raw strings; remaining columns infer types.
fn edge_item_from_csv(
    target_name: &str,
    headers: &csv::StringRecord,
    record: &csv::StringRecord,
) -> Result<BatchItem, HttpError> {
    let src = record.get(0).ok_or_else(|| {
        HttpError::BadRequest("CSV edge rows must start with the source VID".into())
    })?;
    let dst = record
        .get(1)
        .ok_or_else(|| HttpError::BadRequest("CSV edge rows must have a destination VID".into()))?;
    let mut properties = HashMap::new();
    for (index, name) in headers.iter().enumerate().skip(2) {
        let raw = record.get(index).unwrap_or("");
        properties.insert(name.to_string(), parse_cell(raw));
    }
    Ok(BatchItem::Edge(EdgeData {
        edge_type: target_name.to_string(),
        src_vid: serde_json::Value::String(src.to_string()),
        dst_vid: serde_json::Value::String(dst.to_string()),
        properties,
    }))
}

/// Build one vertex batch item from a JSON object, keeping native value
/// types. The identifier accepts strings and numbers.
fn vertex_item_from_json_object(
    target_name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<BatchItem, HttpError> {
    let vid = obj
        .get("_id")
        .cloned()
        .filter(|id| {
            matches!(
                id,
                serde_json::Value::String(_) | serde_json::Value::Number(_)
            )
        })
        .ok_or_else(|| {
            HttpError::BadRequest("JSON vertex objects must have a string or numeric _id".into())
        })?;
    let mut properties = HashMap::new();
    for (key, value) in obj {
        if key == "_id" {
            continue;
        }
        properties.insert(key.clone(), value.clone());
    }
    Ok(BatchItem::Vertex(VertexData {
        vid,
        tag: target_name.to_string(),
        properties,
    }))
}

/// Build one edge batch item from a JSON object, keeping native value
/// types for properties.
fn edge_item_from_json_object(
    target_name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<BatchItem, HttpError> {
    let src = obj
        .get("_src")
        .cloned()
        .filter(|id| {
            matches!(
                id,
                serde_json::Value::String(_) | serde_json::Value::Number(_)
            )
        })
        .ok_or_else(|| {
            HttpError::BadRequest("JSON edge objects must have a string or numeric _src".into())
        })?;
    let dst = obj
        .get("_dst")
        .cloned()
        .filter(|id| {
            matches!(
                id,
                serde_json::Value::String(_) | serde_json::Value::Number(_)
            )
        })
        .ok_or_else(|| {
            HttpError::BadRequest("JSON edge objects must have a string or numeric _dst".into())
        })?;
    let mut properties = HashMap::new();
    for (key, value) in obj {
        if key == "_src" || key == "_dst" {
            continue;
        }
        properties.insert(key.clone(), value.clone());
    }
    Ok(BatchItem::Edge(EdgeData {
        edge_type: target_name.to_string(),
        src_vid: src,
        dst_vid: dst,
        properties,
    }))
}

/// Parse the uploaded file bytes into typed batch items.
fn parse_items(
    format: &str,
    target_type: &str,
    target_name: &str,
    data: &[u8],
) -> Result<Vec<BatchItem>, HttpError> {
    if !matches!(target_type, "tag" | "vertex" | "edge") {
        return Err(HttpError::BadRequest(format!(
            "Unsupported target_type: {target_type} (expected 'tag', 'vertex' or 'edge')"
        )));
    }
    let vertex = !matches!(target_type, "edge");
    match format {
        "csv" => {
            let mut reader = csv::ReaderBuilder::new()
                .has_headers(true)
                .from_reader(data);
            let headers = reader
                .headers()
                .map_err(|e| HttpError::BadRequest(format!("CSV header parse failed: {e}")))?
                .clone();
            let mut items = Vec::new();
            for record in reader.records() {
                let record = record
                    .map_err(|e| HttpError::BadRequest(format!("CSV record parse failed: {e}")))?;
                let item = if vertex {
                    vertex_item_from_csv(target_name, &headers, &record)?
                } else {
                    edge_item_from_csv(target_name, &headers, &record)?
                };
                items.push(item);
            }
            Ok(items)
        }
        "json" | "jsonl" => {
            let text = std::str::from_utf8(data)
                .map_err(|e| HttpError::BadRequest(format!("File is not valid UTF-8: {e}")))?;
            let mut objects: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
            if format == "jsonl" {
                for (idx, line) in text.lines().enumerate() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let obj: serde_json::Map<String, serde_json::Value> =
                        serde_json::from_str(line).map_err(|e| {
                            HttpError::BadRequest(format!(
                                "JSONL line {} parse failed: {e}",
                                idx + 1
                            ))
                        })?;
                    objects.push(obj);
                }
            } else {
                let value: serde_json::Value = serde_json::from_str(text)
                    .map_err(|e| HttpError::BadRequest(format!("JSON parse failed: {e}")))?;
                match value {
                    serde_json::Value::Array(items) => {
                        for item in items {
                            objects.push(item.as_object().cloned().ok_or_else(|| {
                                HttpError::BadRequest(
                                    "JSON array items must be objects".to_string(),
                                )
                            })?);
                        }
                    }
                    serde_json::Value::Object(_) => {
                        objects.push(value.as_object().expect("checked above").clone());
                    }
                    _ => {
                        return Err(HttpError::BadRequest(
                            "JSON import expects an object or an array of objects".to_string(),
                        ))
                    }
                }
            }
            objects
                .iter()
                .map(|obj| {
                    if vertex {
                        vertex_item_from_json_object(target_name, obj)
                    } else {
                        edge_item_from_json_object(target_name, obj)
                    }
                })
                .collect()
        }
        other => Err(HttpError::BadRequest(format!(
            "Unsupported format: {other} (expected 'csv', 'json' or 'jsonl')"
        ))),
    }
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
    if !matches!(target_type.as_str(), "tag" | "vertex" | "edge") {
        return Err(HttpError::BadRequest(format!(
            "Unsupported target_type: {} (expected 'tag', 'vertex' or 'edge')",
            target_type
        )));
    }
    let file_data =
        file_data.ok_or_else(|| HttpError::BadRequest("Missing 'file' field".to_string()))?;

    // The requested space is authoritative: it must exist, and the caller
    // needs write permission on it, the same verdict the statement channel
    // reaches for inserts.
    let space_id = {
        let storage = state.server.get_storage();
        let storage = storage.read();
        storage.get_space_id(&space).map_err(|_| {
            HttpError::NotFound(format!("The graph space does not exist: {}", space))
        })?
    };
    let session = super::authz::require_space_write(&state, session_id, space_id as i64)?;
    session.charge();

    let items = parse_items(&format, &target_type, &target_name, &file_data)?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let group_size = batch_size.unwrap_or(1000).max(1);

    let outcome = state
        .server
        .get_batch_manager()
        .execute_items(&space, items, group_size, None)
        .await;
    let completed_at = chrono::Utc::now().to_rfc3339();

    let (success, rows_imported, rows_failed, message) = match outcome {
        Ok(result) => {
            let rows_imported = result.vertices_inserted + result.edges_inserted;
            let rows_failed = result.errors.len();
            let success = rows_failed == 0;
            let message = if success {
                format!("Imported {rows_imported} rows into {target_type} '{target_name}'")
            } else {
                format!(
                    "Imported {rows_imported} rows, {rows_failed} failed (group size {group_size})"
                )
            };
            (success, rows_imported, rows_failed, message)
        }
        Err(e) => {
            return Err(HttpError::InternalError(format!(
                "Failed to import file: {}",
                e
            )))
        }
    };

    let job_id = uuid::Uuid::new_v4().to_string();
    state.server.import_jobs().record(ImportJobRecord {
        job_id: job_id.clone(),
        space: space.clone(),
        format: format.clone(),
        target_type: target_type.clone(),
        target_name: target_name.clone(),
        status: if success { "completed" } else { "failed" }.to_string(),
        rows_imported,
        rows_failed,
        started_at,
        completed_at,
    });

    Ok(JsonResponse(ImportResponse {
        success,
        job_id,
        message,
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
        (status = 404, description = "Unknown import job")
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
    State(state): State<AppState<S>>,
    Path(job_id): Path<String>,
) -> Result<JsonResponse<ImportStatusResponse>, HttpError> {
    match state.server.import_jobs().get(&job_id) {
        Some(record) => Ok(JsonResponse(ImportStatusResponse {
            job_id: record.job_id,
            status: record.status,
            rows_imported: record.rows_imported,
            rows_failed: record.rows_failed,
            started_at: Some(record.started_at),
            completed_at: Some(record.completed_at),
        })),
        None => Err(HttpError::NotFound(format!(
            "Import job does not exist: {}",
            job_id
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cell_keeps_types() {
        assert_eq!(parse_cell(""), serde_json::Value::Null);
        assert_eq!(parse_cell("30"), serde_json::json!(30));
        assert_eq!(parse_cell("-4"), serde_json::json!(-4));
        assert_eq!(parse_cell("0.5"), serde_json::json!(0.5));
        assert_eq!(parse_cell("true"), serde_json::Value::Bool(true));
        assert_eq!(parse_cell("FALSE"), serde_json::Value::Bool(false));
        assert_eq!(
            parse_cell("Alice"),
            serde_json::Value::String("Alice".to_string())
        );
    }

    #[test]
    fn parse_csv_vertex_items_keep_vid_string_and_type_properties() {
        let csv = "vid,name,age\n1,Alice,30\n2,Bob,25\n";
        let items = parse_items("csv", "tag", "Person", csv.as_bytes()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            BatchItem::Vertex(v) => {
                assert_eq!(v.vid, serde_json::json!("1"));
                assert_eq!(v.tag, "Person");
                assert_eq!(v.properties.get("name"), Some(&serde_json::json!("Alice")));
                assert_eq!(v.properties.get("age"), Some(&serde_json::json!(30)));
            }
            other => panic!("expected vertex item, got {other:?}"),
        }
    }

    #[test]
    fn parse_csv_edge_items() {
        let csv = "src,dst,weight\n1,2,0.5\n3,4,1.5\n";
        let items = parse_items("csv", "edge", "Follows", csv.as_bytes()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            BatchItem::Edge(e) => {
                assert_eq!(e.edge_type, "Follows");
                assert_eq!(e.src_vid, serde_json::json!("1"));
                assert_eq!(e.dst_vid, serde_json::json!("2"));
                assert_eq!(e.properties.get("weight"), Some(&serde_json::json!(0.5)));
            }
            other => panic!("expected edge item, got {other:?}"),
        }
    }

    #[test]
    fn parse_json_array_vertex_items_keep_native_types() {
        let json = r#"[{"_id":"v1","name":"Alice","age":30,"active":true}]"#;
        let items = parse_items("json", "tag", "Person", json.as_bytes()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            BatchItem::Vertex(v) => {
                assert_eq!(v.vid, serde_json::json!("v1"));
                assert_eq!(v.properties.get("age"), Some(&serde_json::json!(30)));
                assert_eq!(
                    v.properties.get("active"),
                    Some(&serde_json::Value::Bool(true))
                );
            }
            other => panic!("expected vertex item, got {other:?}"),
        }
    }

    #[test]
    fn parse_jsonl_edge_items() {
        let jsonl = "{\"_src\":\"1\",\"_dst\":\"2\",\"weight\":0.5}\n{\"_src\":\"3\",\"_dst\":\"4\",\"weight\":1.5}\n";
        let items = parse_items("jsonl", "edge", "Follows", jsonl.as_bytes()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            BatchItem::Edge(e) => {
                assert_eq!(e.src_vid, serde_json::json!("1"));
                assert_eq!(e.dst_vid, serde_json::json!("2"));
            }
            other => panic!("expected edge item, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_unsupported_target_type() {
        assert!(parse_items("csv", "index", "Person", b"vid\n1\n").is_err());
    }

    #[test]
    fn parse_rejects_unsupported_format() {
        assert!(parse_items("yaml", "tag", "Person", b"a: 1").is_err());
    }

    #[test]
    fn parse_rejects_json_edge_without_src() {
        let json = r#"[{"_dst":"2"}]"#;
        assert!(parse_items("json", "edge", "Follows", json.as_bytes()).is_err());
    }

    #[test]
    fn parse_empty_csv_yields_no_items() {
        let items = parse_items("csv", "tag", "Person", b"vid,name\n").unwrap();
        assert!(items.is_empty());
    }
}
