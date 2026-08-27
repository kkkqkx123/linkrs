//! HTTP handler for data import operations
//!
//! Parses an uploaded file (CSV with header, JSON array, or JSONL) into DML
//! statements and loads them through the group-commit batch window:
//! each `batch_size` group of statements shares one write timestamp, one WAL
//! fsync, and one commit point, so a large import amortizes fsync across
//! statements instead of syncing per row.

use axum::{
    extract::{Extension, Multipart, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
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

/// Build one `INSERT VERTEX <tag> (fields) VALUES "<vid>":(values)` statement
/// from a CSV record.
fn vertex_statement_from_csv(
    target_name: &str,
    headers: &csv::StringRecord,
    record: &csv::StringRecord,
) -> Result<String, HttpError> {
    let fields: Vec<&str> = headers.iter().collect();
    let values: Vec<String> = record.iter().map(format_value).collect();
    let vid = record
        .get(0)
        .ok_or_else(|| HttpError::BadRequest("CSV vertex rows must start with the VID".into()))?
        .to_string();
    Ok(format!(
        "INSERT VERTEX {} ({}) VALUES \"{}\":({})",
        target_name,
        fields.join(", "),
        vid,
        values.join(", ")
    ))
}

/// Build one `INSERT EDGE <type> (fields) VALUES "<src>"->"<dst>":(values)`
/// statement from a CSV record. Columns 0/1 are the source/destination VIDs,
/// remaining columns are properties.
fn edge_statement_from_csv(
    target_name: &str,
    headers: &csv::StringRecord,
    record: &csv::StringRecord,
) -> Result<String, HttpError> {
    let src = record.get(0).ok_or_else(|| {
        HttpError::BadRequest("CSV edge rows must start with the source VID".into())
    })?;
    let dst = record
        .get(1)
        .ok_or_else(|| HttpError::BadRequest("CSV edge rows must have a destination VID".into()))?;
    let fields: Vec<&str> = headers.iter().skip(2).collect();
    let values: Vec<String> = record.iter().skip(2).map(format_value).collect();
    Ok(format!(
        "INSERT EDGE {} ({}) VALUES \"{}\"->\"{}\":({})",
        target_name,
        fields.join(", "),
        src,
        dst,
        values.join(", ")
    ))
}

/// Build one INSERT statement from a JSON object. Vertex objects use `_id`
/// as the VID; edge objects use `_src`/`_dst`.
fn statement_from_json_object(
    target_type: &str,
    target_name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, HttpError> {
    match target_type {
        "tag" | "vertex" => {
            let vid = obj
                .get("_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| HttpError::BadRequest("JSON vertex objects must have _id".into()))?;
            let mut fields = Vec::new();
            let mut values = Vec::new();
            for (key, value) in obj {
                if key == "_id" {
                    continue;
                }
                fields.push(key.as_str());
                values.push(json_value_to_gql(value));
            }
            Ok(format!(
                "INSERT VERTEX {} ({}) VALUES \"{}\":({})",
                target_name,
                fields.join(", "),
                vid,
                values.join(", ")
            ))
        }
        "edge" => {
            let src = obj
                .get("_src")
                .and_then(|v| v.as_str())
                .ok_or_else(|| HttpError::BadRequest("JSON edge objects must have _src".into()))?;
            let dst = obj
                .get("_dst")
                .and_then(|v| v.as_str())
                .ok_or_else(|| HttpError::BadRequest("JSON edge objects must have _dst".into()))?;
            let mut fields = Vec::new();
            let mut values = Vec::new();
            for (key, value) in obj {
                if key == "_src" || key == "_dst" {
                    continue;
                }
                fields.push(key.as_str());
                values.push(json_value_to_gql(value));
            }
            Ok(format!(
                "INSERT EDGE {} ({}) VALUES \"{}\"->\"{}\":({})",
                target_name,
                fields.join(", "),
                src,
                dst,
                values.join(", ")
            ))
        }
        other => Err(HttpError::BadRequest(format!(
            "Unsupported target_type: {other} (expected 'tag' or 'edge')"
        ))),
    }
}

fn format_value(value: &str) -> String {
    if value.is_empty() {
        return "NULL".to_string();
    }
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn json_value_to_gql(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(json_value_to_gql).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(_) => {
            format!("\"{}\"", serde_json::to_string(value).unwrap_or_default())
        }
    }
}

/// Parse the uploaded file bytes into DML statements.
fn parse_statements(
    format: &str,
    target_type: &str,
    target_name: &str,
    data: &[u8],
) -> Result<Vec<String>, HttpError> {
    match format {
        "csv" => {
            let mut reader = csv::ReaderBuilder::new()
                .has_headers(true)
                .from_reader(data);
            let headers = reader
                .headers()
                .map_err(|e| HttpError::BadRequest(format!("CSV header parse failed: {e}")))?
                .clone();
            let mut statements = Vec::new();
            for record in reader.records() {
                let record = record
                    .map_err(|e| HttpError::BadRequest(format!("CSV record parse failed: {e}")))?;
                let statement = match target_type {
                    "edge" => edge_statement_from_csv(target_name, &headers, &record)?,
                    _ => vertex_statement_from_csv(target_name, &headers, &record)?,
                };
                statements.push(statement);
            }
            Ok(statements)
        }
        "json" | "jsonl" => {
            let text = std::str::from_utf8(data)
                .map_err(|e| HttpError::BadRequest(format!("File is not valid UTF-8: {e}")))?;
            let mut statements = Vec::new();
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
                    statements.push(statement_from_json_object(target_type, target_name, &obj)?);
                }
            } else {
                let value: serde_json::Value = serde_json::from_str(text)
                    .map_err(|e| HttpError::BadRequest(format!("JSON parse failed: {e}")))?;
                let objects: Vec<&serde_json::Map<String, serde_json::Value>> = match &value {
                    serde_json::Value::Array(items) => items
                        .iter()
                        .map(|item| {
                            item.as_object().ok_or_else(|| {
                                HttpError::BadRequest(
                                    "JSON array items must be objects".to_string(),
                                )
                            })
                        })
                        .collect::<Result<_, _>>()?,
                    serde_json::Value::Object(_) => {
                        vec![value.as_object().expect("checked above")]
                    }
                    _ => {
                        return Err(HttpError::BadRequest(
                            "JSON import expects an object or an array of objects".to_string(),
                        ))
                    }
                };
                for obj in objects {
                    statements.push(statement_from_json_object(target_type, target_name, obj)?);
                }
            }
            Ok(statements)
        }
        other => Err(HttpError::BadRequest(format!(
            "Unsupported format: {other} (expected 'csv', 'json' or 'jsonl')"
        ))),
    }
}

pub async fn import_file<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + crate::storage::AutoCommitBatchOps
        + crate::storage::AutoCommitGroupOps
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

    let statements = parse_statements(&format, &target_type, &target_name, &file_data)?;
    if statements.is_empty() {
        return Ok(JsonResponse(ImportResponse {
            success: true,
            message: "File contained no importable rows".to_string(),
            rows_imported: 0,
            rows_failed: 0,
        }));
    }

    let group_size = batch_size.unwrap_or(1000).max(1);
    let outcomes = state
        .server
        .get_graph_service()
        .execute_batch_grouped(session_id, &statements, group_size)
        .await;

    let mut rows_imported = 0;
    let mut rows_failed = 0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_vertex_statements() {
        let csv = "vid,name,age\n1,Alice,30\n2,Bob,25\n";
        let statements = parse_statements("csv", "tag", "Person", csv.as_bytes()).unwrap();
        assert_eq!(statements.len(), 2);
        assert_eq!(
            statements[0],
            "INSERT VERTEX Person (vid, name, age) VALUES \"1\":(\"1\", \"Alice\", \"30\")"
        );
        assert_eq!(
            statements[1],
            "INSERT VERTEX Person (vid, name, age) VALUES \"2\":(\"2\", \"Bob\", \"25\")"
        );
    }

    #[test]
    fn parse_csv_edge_statements() {
        let csv = "src,dst,weight\n1,2,0.5\n3,4,1.5\n";
        let statements = parse_statements("csv", "edge", "Follows", csv.as_bytes()).unwrap();
        assert_eq!(statements.len(), 2);
        assert_eq!(
            statements[0],
            "INSERT EDGE Follows (weight) VALUES \"1\"->\"2\":(\"0.5\")"
        );
    }

    #[test]
    fn parse_json_array_vertex_statements() {
        let json = r#"[{"_id":"v1","name":"Alice","age":30},{"_id":"v2","name":"Bob","age":25}]"#;
        let statements = parse_statements("json", "tag", "Person", json.as_bytes()).unwrap();
        assert_eq!(statements.len(), 2);
        // serde_json::Map iterates keys in sorted order.
        assert_eq!(
            statements[0],
            "INSERT VERTEX Person (age, name) VALUES \"v1\":(30, \"Alice\")"
        );
    }

    #[test]
    fn parse_jsonl_edge_statements() {
        let jsonl = "{\"_src\":\"1\",\"_dst\":\"2\",\"weight\":0.5}\n{\"_src\":\"3\",\"_dst\":\"4\",\"weight\":1.5}\n";
        let statements = parse_statements("jsonl", "edge", "Follows", jsonl.as_bytes()).unwrap();
        assert_eq!(statements.len(), 2);
        assert_eq!(
            statements[0],
            "INSERT EDGE Follows (weight) VALUES \"1\"->\"2\":(0.5)"
        );
    }

    #[test]
    fn parse_escapes_quotes_in_values() {
        let csv = "vid,name\n1,\"Al\"\"ice\"\n";
        let statements = parse_statements("csv", "tag", "Person", csv.as_bytes()).unwrap();
        assert_eq!(statements.len(), 1);
        // The embedded quote is escaped for the INSERT statement literal.
        assert!(statements[0].contains("Al\\\"ice"), "{}", statements[0]);
    }

    #[test]
    fn parse_rejects_unsupported_format() {
        assert!(parse_statements("yaml", "tag", "Person", b"a: 1").is_err());
    }

    #[test]
    fn parse_rejects_json_edge_without_src() {
        let json = r#"[{"_dst":"2"}]"#;
        assert!(parse_statements("json", "edge", "Follows", json.as_bytes()).is_err());
    }

    #[test]
    fn parse_empty_csv_yields_no_statements() {
        let statements = parse_statements("csv", "tag", "Person", b"vid,name\n").unwrap();
        assert!(statements.is_empty());
    }
}
