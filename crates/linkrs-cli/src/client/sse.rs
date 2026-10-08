//! SSE (`text/event-stream`) parsing for streaming query responses.
//!
//! Split out of `http_client` so the event state machine can evolve without
//! touching request plumbing. The transport (`HttpClient`) only fetches the
//! raw stream text; everything below turns `event:`/`data:` lines into one
//! [`QueryResult`].

use std::collections::HashMap;

use crate::client::types::QueryResult;
use crate::utils::error::{CliError, Result};

/// Incremental SSE parser collecting one streaming query result.
pub struct SseParser {
    columns: Vec<String>,
    rows: Vec<HashMap<String, serde_json::Value>>,
    execution_time_ms: u64,
    rows_scanned: u64,
    stream_error: Option<String>,
    current_event: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            execution_time_ms: 0,
            rows_scanned: 0,
            stream_error: None,
            current_event: String::new(),
        }
    }

    /// Feed one raw stream line; blank lines and comments carry no payload.
    pub fn feed_line(&mut self, line: &str) {
        if let Some(name) = line.strip_prefix("event:") {
            self.current_event = name.trim().to_string();
            return;
        }
        let Some(payload) = line.strip_prefix("data:") else {
            return;
        };
        let payload = payload.trim();
        if payload.is_empty() {
            return;
        }
        let value: serde_json::Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => return,
        };
        match self.current_event.as_str() {
            "schema" => {
                if let Some(cols) = value.get("columns").and_then(|c| c.as_array()) {
                    self.columns = cols
                        .iter()
                        .filter_map(|c| c.as_str().map(|s| s.to_string()))
                        .collect();
                } else if let Some(cols) = value.get("schema").and_then(|c| c.as_array()) {
                    self.columns = cols
                        .iter()
                        .filter_map(|c| c.as_str().map(|s| s.to_string()))
                        .collect();
                }
            }
            "row" | "data" => {
                let obj = value.get("row").unwrap_or(&value);
                if let Some(map) = obj.as_object() {
                    self.rows
                        .push(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
                }
            }
            "metadata" | "done" => {
                if let Some(ms) = value.get("execution_time_ms").and_then(|v| v.as_u64()) {
                    self.execution_time_ms = ms;
                }
                if let Some(scanned) = value.get("rows_scanned").and_then(|v| v.as_u64()) {
                    self.rows_scanned = scanned;
                }
            }
            "error" => {
                let message = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("stream error")
                    .to_string();
                let code = value
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("STREAM_ERROR");
                self.stream_error = Some(format!("{}: {}", code, message));
            }
            _ => {}
        }
    }

    /// Consume the parser into the collected result, or the stream error.
    pub fn finish(self) -> Result<QueryResult> {
        if let Some(error) = self.stream_error {
            return Err(CliError::query(error));
        }

        let row_count = self.rows.len();
        Ok(QueryResult {
            columns: self.columns,
            rows: self.rows,
            row_count,
            execution_time_ms: self.execution_time_ms,
            rows_scanned: self.rows_scanned,
            error: None,
        })
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<QueryResult> {
        let mut parser = SseParser::new();
        for line in text.lines() {
            parser.feed_line(line);
        }
        parser.finish()
    }

    #[test]
    fn collects_schema_rows_and_metadata() {
        let result = parse(
            "event: schema\ndata: {\"columns\": [\"name\", \"age\"]}\n\
             event: row\ndata: {\"row\": {\"name\": \"Alice\", \"age\": 30}, \"index\": 0}\n\
             event: metadata\ndata: {\"execution_time_ms\": 12, \"rows_scanned\": 42}\n",
        )
        .expect("stream should parse");
        assert_eq!(result.columns, vec!["name", "age"]);
        assert_eq!(result.row_count, 1);
        assert_eq!(result.execution_time_ms, 12);
        assert_eq!(result.rows_scanned, 42);
        assert!(result.error.is_none());
    }

    #[test]
    fn surfaces_stream_error() {
        let err = parse("event: error\ndata: {\"code\": \"QUERY_ERROR\", \"message\": \"boom\"}\n")
            .expect_err("stream error should fail");
        assert!(err.to_string().contains("QUERY_ERROR: boom"));
    }

    #[test]
    fn ignores_malformed_payloads() {
        let result = parse("event: row\ndata: not-json\n\nevent: done\ndata: {}\n")
            .expect("malformed lines are skipped");
        assert!(result.rows.is_empty());
        assert_eq!(result.row_count, 0);
    }
}
