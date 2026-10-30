//! File export for embedded sessions.
//!
//! Server-side `/export` streams a fresh query execution as `csv`/`jsonl`.
//! This is the embedded equivalent: execute the query in-process and write
//! the same two formats to a local file. Programmatic bulk import is
//! covered by [`crate::embedded::BatchInserter`]; this module covers the
//! file-export half so embedded users do not need the HTTP server.

use super::Session;
use crate::api_core::{CoreError, CoreResult};
use linkrs_storage::StorageClient;
use std::path::Path;

/// Export file format, matching server `/export?format=` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Jsonl,
}

impl ExportFormat {
    /// Parse `csv` / `jsonl` (case-insensitive).
    pub fn parse(raw: &str) -> CoreResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "csv" => Ok(Self::Csv),
            "jsonl" | "ndjson" | "json-lines" => Ok(Self::Jsonl),
            other => Err(CoreError::InvalidParameter(format!(
                "unsupported export format '{other}'; use csv or jsonl"
            ))),
        }
    }
}

fn escape_csv_cell(text: &str) -> String {
    if text.contains(',') || text.contains('"') || text.contains('\n') || text.contains('\r') {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

fn value_to_cell(value: &linkrs_core::Value) -> String {
    match value {
        linkrs_core::Value::Null(_) => String::new(),
        linkrs_core::Value::Bool(b) => b.to_string(),
        linkrs_core::Value::String(s) => s.to_string(),
        other => format!("{other}"),
    }
}

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Execute `query` and write the result to `path` as `csv` or `jsonl`.
    ///
    /// Returns the number of rows written, excluding the CSV header row.
    pub fn export_query_to_file(
        &self,
        query: &str,
        path: impl AsRef<Path>,
        format: ExportFormat,
    ) -> CoreResult<usize> {
        let result = self.execute(query)?;
        self.export_result_to_file(&result, path, format)
    }

    /// Write an already-executed result to `path`.
    pub fn export_result_to_file(
        &self,
        result: &crate::embedded::result::QueryResult,
        path: impl AsRef<Path>,
        format: ExportFormat,
    ) -> CoreResult<usize> {
        use std::io::Write;
        let mut file = std::fs::File::create(path.as_ref())
            .map_err(|e| CoreError::Internal(format!("failed to create export file: {e}")))?;
        let rows = result.rows().len();
        match format {
            ExportFormat::Csv => {
                let header: Vec<String> = result
                    .columns()
                    .iter()
                    .map(|c| escape_csv_cell(c))
                    .collect();
                writeln!(file, "{}", header.join(",")).map_err(|e| {
                    CoreError::Internal(format!("failed to write export file: {e}"))
                })?;
                for row in result.rows() {
                    let cells: Vec<String> = result
                        .columns()
                        .iter()
                        .map(|col| row.get(col).map(value_to_cell).unwrap_or_default())
                        .map(|cell| escape_csv_cell(&cell))
                        .collect();
                    writeln!(file, "{}", cells.join(",")).map_err(|e| {
                        CoreError::Internal(format!("failed to write export file: {e}"))
                    })?;
                }
            }
            ExportFormat::Jsonl => {
                for row in result.rows() {
                    let line = serde_json::to_string(row).map_err(|e| {
                        CoreError::Internal(format!("JSON serialization failed: {e}"))
                    })?;
                    writeln!(file, "{line}").map_err(|e| {
                        CoreError::Internal(format!("failed to write export file: {e}"))
                    })?;
                }
            }
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::database::GraphDatabase;
    use linkrs_storage::MockStorage;

    #[test]
    fn export_format_parses_known_values() {
        assert_eq!(ExportFormat::parse("csv").unwrap(), ExportFormat::Csv);
        assert_eq!(ExportFormat::parse("JSONL").unwrap(), ExportFormat::Jsonl);
        assert!(ExportFormat::parse("xml").is_err());
    }

    #[test]
    fn export_result_to_file_writes_csv_and_jsonl() {
        let db = GraphDatabase::<MockStorage>::open_test().expect("test database opens");
        let session = db.session().expect("session creates");
        let result = session
            .execute("RETURN 1 AS a, 2 AS b")
            .expect("query executes");
        let dir = std::env::temp_dir();
        let csv_path = dir.join("linkrs_embedded_export_test.csv");
        let jsonl_path = dir.join("linkrs_embedded_export_test.jsonl");
        let rows_csv = session
            .export_result_to_file(&result, &csv_path, ExportFormat::Csv)
            .expect("csv export works");
        let rows_jsonl = session
            .export_result_to_file(&result, &jsonl_path, ExportFormat::Jsonl)
            .expect("jsonl export works");
        assert_eq!(rows_csv, result.rows().len());
        assert_eq!(rows_jsonl, result.rows().len());
        let csv_text = std::fs::read_to_string(&csv_path).expect("csv readable");
        assert!(csv_text.contains('a') && csv_text.contains('b'));
        let _ = std::fs::remove_file(&csv_path);
        let _ = std::fs::remove_file(&jsonl_path);
    }
}
