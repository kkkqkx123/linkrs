//! Import/export RPCs (parity with `/v1/import`, `/v1/export`).
//!
//! The HTTP import accepts a multipart file; the RPC carries the file
//! content as text plus the same metadata fields and reuses the grouped
//! batch execution path. Export re-executes the query on the materialized
//! path and returns the encoded body as text.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::error::parse_session_id;
use super::proto::*;
use super::service::GraphDBService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > GraphDBService<S>
{
    pub(crate) async fn handle_import_data(
        &self,
        request: Request<ImportDataRequest>,
    ) -> Result<Response<ImportDataResponse>, Status> {
        let req = request.into_inner();
        if req.space.trim().is_empty() {
            return Err(Status::invalid_argument("space must not be empty"));
        }
        if req.target_name.trim().is_empty() {
            return Err(Status::invalid_argument("target_name must not be empty"));
        }
        let session_id = match req.session_id.filter(|s| !s.is_empty()) {
            Some(raw) => parse_session_id(&raw)?,
            None => return Err(Status::unauthenticated("session_id is required")),
        };
        let statements = parse_import_statements(
            &req.format,
            &req.target_type,
            &req.target_name,
            req.content_text.as_bytes(),
        )?;
        if statements.is_empty() {
            return Ok(Response::new(ImportDataResponse {
                success: true,
                rows_imported: 0,
                rows_failed: 0,
                message: "File contained no importable rows".to_string(),
                error: String::new(),
            }));
        }
        let graph_service = self.app_state.server.get_graph_service();
        let mut imported = 0u64;
        let mut failed = 0u64;
        for stmt in &statements {
            match graph_service
                .execute_with_params(session_id, stmt, None, None)
                .await
            {
                Ok(_) => imported += 1,
                Err(_) => failed += 1,
            }
        }
        let success = failed == 0;
        Ok(Response::new(ImportDataResponse {
            success,
            rows_imported: imported,
            rows_failed: failed,
            message: if success {
                format!(
                    "Imported {imported} rows into {} '{}'",
                    req.target_type, req.target_name
                )
            } else {
                format!("Imported {imported} rows, {failed} failed")
            },
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_get_import_status(
        &self,
        request: Request<GetImportStatusRequest>,
    ) -> Result<Response<GetImportStatusResponse>, Status> {
        let req = request.into_inner();
        Ok(Response::new(GetImportStatusResponse {
            job_id: req.job_id,
            status: "unknown".to_string(),
            rows_imported: 0,
            rows_failed: 0,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_export_data(
        &self,
        request: Request<ExportDataRequest>,
    ) -> Result<Response<ExportDataResponse>, Status> {
        let req = request.into_inner();
        if req.query.trim().is_empty() {
            return Err(Status::invalid_argument("query must not be empty"));
        }
        let format = req.format.to_lowercase();
        if format != "csv" && format != "jsonl" {
            return Err(Status::invalid_argument(
                "Unsupported export format; use csv or jsonl",
            ));
        }
        let session_id = match req.session_id.filter(|s| !s.is_empty()) {
            Some(raw) => parse_session_id(&raw)?,
            None => return Err(Status::unauthenticated("session_id is required")),
        };
        let graph_service = self.app_state.server.get_graph_service();
        match graph_service
            .execute_with_params(session_id, &req.query, None, None)
            .await
        {
            Ok(result) => {
                let (columns, rows) = materialized_rows(&result);
                let content = if format == "csv" {
                    encode_csv(&columns, &rows)
                } else {
                    encode_jsonl(&rows)
                };
                Ok(Response::new(ExportDataResponse {
                    success: true,
                    content_text: content,
                    row_count: rows.len() as u64,
                    error: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(ExportDataResponse {
                success: false,
                content_text: String::new(),
                row_count: 0,
                error: e.to_string(),
            })),
        }
    }
}

fn parse_import_statements(
    format: &str,
    target_type: &str,
    target_name: &str,
    data: &[u8],
) -> Result<Vec<String>, Status> {
    match format {
        "csv" => parse_csv_statements(target_type, target_name, data),
        "json" | "jsonl" => parse_json_statements(format, target_type, target_name, data),
        other => Err(Status::invalid_argument(format!(
            "Unsupported format: {other} (expected 'csv', 'json' or 'jsonl')"
        ))),
    }
}

fn parse_csv_statements(
    target_type: &str,
    target_name: &str,
    data: &[u8],
) -> Result<Vec<String>, Status> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(data);
    let headers = reader
        .headers()
        .map_err(|e| Status::invalid_argument(format!("CSV header parse failed: {e}")))?
        .clone();
    let mut statements = Vec::new();
    for record in reader.records() {
        let record = record
            .map_err(|e| Status::invalid_argument(format!("CSV record parse failed: {e}")))?;
        if target_type == "edge" {
            let src = record.get(0).ok_or_else(|| {
                Status::invalid_argument("CSV edge rows must start with the source VID")
            })?;
            let dst = record.get(1).ok_or_else(|| {
                Status::invalid_argument("CSV edge rows must have a destination VID")
            })?;
            let fields: Vec<&str> = headers.iter().skip(2).collect();
            let values: Vec<String> = record.iter().skip(2).map(csv_value).collect();
            statements.push(format!(
                "INSERT EDGE {} ({}) VALUES \"{}\"->\"{}\":({})",
                target_name,
                fields.join(", "),
                src,
                dst,
                values.join(", ")
            ));
        } else {
            let vid = record.get(0).ok_or_else(|| {
                Status::invalid_argument("CSV vertex rows must start with the VID")
            })?;
            let fields: Vec<&str> = headers.iter().collect();
            let values: Vec<String> = record.iter().map(csv_value).collect();
            statements.push(format!(
                "INSERT VERTEX {} ({}) VALUES \"{}\":({})",
                target_name,
                fields.join(", "),
                vid,
                values.join(", ")
            ));
        }
    }
    Ok(statements)
}

fn csv_value(value: &str) -> String {
    if value.is_empty() {
        return "NULL".to_string();
    }
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn parse_json_statements(
    format: &str,
    target_type: &str,
    target_name: &str,
    data: &[u8],
) -> Result<Vec<String>, Status> {
    let text = std::str::from_utf8(data)
        .map_err(|e| Status::invalid_argument(format!("Content is not valid UTF-8: {e}")))?;
    let mut objects: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
    if format == "jsonl" {
        for (idx, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(line)
                .map_err(|e| {
                    Status::invalid_argument(format!("JSONL line {} parse failed: {e}", idx + 1))
                })?;
            objects.push(obj);
        }
    } else {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| Status::invalid_argument(format!("JSON parse failed: {e}")))?;
        match value {
            serde_json::Value::Array(items) => {
                for item in items {
                    match item {
                        serde_json::Value::Object(map) => objects.push(map),
                        _ => {
                            return Err(Status::invalid_argument(
                                "JSON array items must be objects",
                            ));
                        }
                    }
                }
            }
            serde_json::Value::Object(map) => objects.push(map),
            _ => {
                return Err(Status::invalid_argument(
                    "JSON import expects an object or an array of objects",
                ));
            }
        }
    }
    objects
        .iter()
        .map(|obj| json_object_statement(target_type, target_name, obj))
        .collect()
}

fn json_object_statement(
    target_type: &str,
    target_name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, Status> {
    match target_type {
        "tag" | "vertex" => {
            let vid = obj
                .get("_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Status::invalid_argument("JSON vertex objects must have _id"))?;
            let mut fields = Vec::new();
            let mut values = Vec::new();
            for (key, value) in obj {
                if key == "_id" {
                    continue;
                }
                fields.push(key.as_str());
                values.push(json_gql_value(value));
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
                .ok_or_else(|| Status::invalid_argument("JSON edge objects must have _src"))?;
            let dst = obj
                .get("_dst")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Status::invalid_argument("JSON edge objects must have _dst"))?;
            let mut fields = Vec::new();
            let mut values = Vec::new();
            for (key, value) in obj {
                if key == "_src" || key == "_dst" {
                    continue;
                }
                fields.push(key.as_str());
                values.push(json_gql_value(value));
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
        other => Err(Status::invalid_argument(format!(
            "Unsupported target_type: {other} (expected 'tag' or 'edge')"
        ))),
    }
}

fn json_gql_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        serde_json::Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(json_gql_value).collect();
            format!("[{}]", rendered.join(", "))
        }
        serde_json::Value::Object(_) => {
            format!("\"{}\"", serde_json::to_string(value).unwrap_or_default())
        }
    }
}

fn materialized_rows(
    result: &linkrs_api::api_core::QueryResult,
) -> (Vec<String>, Vec<serde_json::Value>) {
    use linkrs_query::executor::base::ExecutionResult;
    match &result.execution {
        ExecutionResult::DataSet { data } => {
            let columns = data.col_names.clone();
            let rows = data
                .rows
                .iter()
                .map(|row| {
                    let mut map = serde_json::Map::new();
                    for (idx, col) in columns.iter().enumerate() {
                        let json = row
                            .get(idx)
                            .map(|value| crate::value::to_json(value.clone()))
                            .unwrap_or(serde_json::Value::Null);
                        map.insert(col.clone(), json);
                    }
                    serde_json::Value::Object(map)
                })
                .collect();
            (columns, rows)
        }
        _ => (Vec::new(), Vec::new()),
    }
}

fn escape_csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn cell_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

fn encode_csv(columns: &[String], rows: &[serde_json::Value]) -> String {
    let mut out = columns
        .iter()
        .map(|c| escape_csv_field(c))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    for row in rows {
        let line = columns
            .iter()
            .map(|col| {
                let text = row.get(col).map(cell_text).unwrap_or_default();
                escape_csv_field(&text)
            })
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn encode_jsonl(rows: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for row in rows {
        out.push_str(&serde_json::to_string(row).unwrap_or_else(|_| "{}".to_string()));
        out.push('\n');
    }
    out
}
