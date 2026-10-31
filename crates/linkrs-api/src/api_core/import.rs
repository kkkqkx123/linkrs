//! Reusable file import for CSV, JSON, and JSONL row payloads.
//!
//! The server upload handler and the embedded `Session::import_file` share
//! this module so both transports turn file bytes into the same INSERT
//! statements and count failures identically. Parsing is best-effort per
//! row: a malformed row is recorded with its input line and skipped instead
//! of aborting the whole file.

use crate::api_core::error::{CoreError, CoreResult};
use serde::{Deserialize, Serialize};

/// File format accepted by the importer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Csv,
    Json,
    Jsonl,
}

impl ImportFormat {
    /// Parse `csv`, `json`, or `jsonl` (case-insensitive).
    pub fn parse(raw: &str) -> CoreResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            "jsonl" | "ndjson" | "json-lines" => Ok(Self::Jsonl),
            other => Err(CoreError::InvalidParameter(format!(
                "unsupported import format '{other}'; use csv, json or jsonl"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Jsonl => "jsonl",
        }
    }
}

/// Target every imported row is loaded into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTarget {
    Vertex,
    Edge,
}

impl ImportTarget {
    /// Parse `tag`/`vertex` or `edge` (case-insensitive).
    pub fn parse(raw: &str) -> CoreResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tag" | "vertex" => Ok(Self::Vertex),
            "edge" => Ok(Self::Edge),
            other => Err(CoreError::InvalidParameter(format!(
                "unsupported import target '{other}'; use tag or edge"
            ))),
        }
    }

    pub fn is_edge(self) -> bool {
        matches!(self, Self::Edge)
    }
}

/// Outcome of an import run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportStats {
    pub rows_imported: usize,
    pub rows_failed: usize,
}

impl ImportStats {
    pub fn total_rows(&self) -> usize {
        self.rows_imported + self.rows_failed
    }
}

/// One parsed row and the input line it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRow {
    pub line: usize,
    pub statement: String,
}

/// A row that could not be turned into a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFailure {
    pub line: usize,
    pub message: String,
}

/// Parsed file contents: executable rows plus per-row failures.
#[derive(Debug, Clone, Default)]
pub struct ParsedImport {
    pub rows: Vec<ImportRow>,
    pub failures: Vec<ImportFailure>,
}

impl ParsedImport {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.failures.is_empty()
    }
}

/// Parse file bytes into INSERT statements.
///
/// Structural problems (unknown format, unreadable CSV header, invalid
/// UTF-8, a JSON document that is neither an object nor an array) return
/// an error. Problems scoped to a single row are collected in
/// [`ParsedImport::failures`] so callers can count them and keep going.
pub fn parse_import(
    format: ImportFormat,
    target: ImportTarget,
    target_name: &str,
    data: &[u8],
) -> CoreResult<ParsedImport> {
    if target_name.trim().is_empty() {
        return Err(CoreError::InvalidParameter(
            "import target name must not be empty".to_string(),
        ));
    }
    match format {
        ImportFormat::Csv => parse_csv(target, target_name, data),
        ImportFormat::Json => parse_json(target, target_name, data),
        ImportFormat::Jsonl => parse_jsonl(target, target_name, data),
    }
}

fn parse_csv(target: ImportTarget, target_name: &str, data: &[u8]) -> CoreResult<ParsedImport> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(data);
    let headers = reader
        .headers()
        .map_err(|e| CoreError::InvalidParameter(format!("CSV header parse failed: {e}")))?
        .clone();
    let mut parsed = ParsedImport::default();
    for (index, record) in reader.records().enumerate() {
        let record = record
            .map_err(|e| CoreError::InvalidParameter(format!("CSV record parse failed: {e}")))?;
        // The header occupies line 1, so the first data record starts on line 2.
        let line = record
            .position()
            .map(|position| position.line() as usize)
            .unwrap_or(index + 2);
        let statement = match target {
            ImportTarget::Vertex => vertex_statement_from_csv(target_name, &headers, &record),
            ImportTarget::Edge => edge_statement_from_csv(target_name, &headers, &record),
        };
        match statement {
            Ok(statement) => parsed.rows.push(ImportRow { line, statement }),
            Err(error) => parsed.failures.push(ImportFailure {
                line,
                message: error.to_string(),
            }),
        }
    }
    Ok(parsed)
}

fn parse_jsonl(target: ImportTarget, target_name: &str, data: &[u8]) -> CoreResult<ParsedImport> {
    let text = std::str::from_utf8(data)
        .map_err(|e| CoreError::InvalidParameter(format!("file is not valid UTF-8: {e}")))?;
    let mut parsed = ParsedImport::default();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let object: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(trimmed)
        {
            Ok(object) => object,
            Err(error) => {
                parsed.failures.push(ImportFailure {
                    line: line_number,
                    message: format!("line {line_number}: JSON parse failed: {error}"),
                });
                continue;
            }
        };
        match statement_from_json_object(target, target_name, &object) {
            Ok(statement) => parsed.rows.push(ImportRow {
                line: line_number,
                statement,
            }),
            Err(error) => parsed.failures.push(ImportFailure {
                line: line_number,
                message: format!("line {line_number}: {error}"),
            }),
        }
    }
    Ok(parsed)
}

fn parse_json(target: ImportTarget, target_name: &str, data: &[u8]) -> CoreResult<ParsedImport> {
    let text = std::str::from_utf8(data)
        .map_err(|e| CoreError::InvalidParameter(format!("file is not valid UTF-8: {e}")))?;
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| CoreError::InvalidParameter(format!("JSON parse failed: {e}")))?;

    let mut parsed = ParsedImport::default();
    match value {
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                push_json_item(&mut parsed, target, target_name, item, index + 1);
            }
        }
        serde_json::Value::Object(object) => {
            push_json_item(
                &mut parsed,
                target,
                target_name,
                &serde_json::Value::Object(object),
                1,
            );
        }
        _ => {
            return Err(CoreError::InvalidParameter(
                "JSON import expects an object or an array of objects".to_string(),
            ))
        }
    }
    Ok(parsed)
}

fn push_json_item(
    parsed: &mut ParsedImport,
    target: ImportTarget,
    target_name: &str,
    item: &serde_json::Value,
    line: usize,
) {
    let Some(object) = item.as_object() else {
        parsed.failures.push(ImportFailure {
            line,
            message: format!("item {line}: JSON array items must be objects"),
        });
        return;
    };
    match statement_from_json_object(target, target_name, object) {
        Ok(statement) => parsed.rows.push(ImportRow { line, statement }),
        Err(error) => parsed.failures.push(ImportFailure {
            line,
            message: format!("item {line}: {error}"),
        }),
    }
}

/// Build one `INSERT VERTEX <tag> (fields) VALUES "<vid>":(values)` statement
/// from a CSV record. Column 0 is the VID; remaining columns are properties.
fn vertex_statement_from_csv(
    target_name: &str,
    headers: &csv::StringRecord,
    record: &csv::StringRecord,
) -> CoreResult<String> {
    let vid = record.get(0).ok_or_else(|| {
        CoreError::InvalidParameter("CSV vertex rows must start with the VID".into())
    })?;
    let fields: Vec<&str> = headers.iter().skip(1).collect();
    let values: Vec<String> = record.iter().skip(1).map(format_value).collect();
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
) -> CoreResult<String> {
    let src = record.get(0).ok_or_else(|| {
        CoreError::InvalidParameter("CSV edge rows must start with the source VID".into())
    })?;
    let dst = record.get(1).ok_or_else(|| {
        CoreError::InvalidParameter("CSV edge rows must have a destination VID".into())
    })?;
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
    target: ImportTarget,
    target_name: &str,
    object: &serde_json::Map<String, serde_json::Value>,
) -> CoreResult<String> {
    match target {
        ImportTarget::Vertex => {
            let vid = object
                .get("_id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    CoreError::InvalidParameter("JSON vertex objects must have _id".into())
                })?;
            let (fields, values) = json_properties(object, &["_id"]);
            Ok(format!(
                "INSERT VERTEX {} ({}) VALUES \"{}\":({})",
                target_name,
                fields.join(", "),
                vid,
                values.join(", ")
            ))
        }
        ImportTarget::Edge => {
            let src = object
                .get("_src")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    CoreError::InvalidParameter("JSON edge objects must have _src".into())
                })?;
            let dst = object
                .get("_dst")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    CoreError::InvalidParameter("JSON edge objects must have _dst".into())
                })?;
            let (fields, values) = json_properties(object, &["_src", "_dst"]);
            Ok(format!(
                "INSERT EDGE {} ({}) VALUES \"{}\"->\"{}\":({})",
                target_name,
                fields.join(", "),
                src,
                dst,
                values.join(", ")
            ))
        }
    }
}

fn json_properties(
    object: &serde_json::Map<String, serde_json::Value>,
    excluded: &[&str],
) -> (Vec<String>, Vec<String>) {
    let mut fields = Vec::new();
    let mut values = Vec::new();
    for (key, value) in object {
        if excluded.contains(&key.as_str()) {
            continue;
        }
        fields.push(key.clone());
        values.push(json_value_to_gql(value));
    }
    (fields, values)
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
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => format!("\"{}\"", value.replace('"', "\\\"")),
        serde_json::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(json_value_to_gql).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(_) => {
            format!("\"{}\"", serde_json::to_string(value).unwrap_or_default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(format: ImportFormat, target: ImportTarget, name: &str, data: &[u8]) -> ParsedImport {
        parse_import(format, target, name, data).expect("import parses")
    }

    #[test]
    fn formats_and_targets_parse_known_values() {
        assert_eq!(ImportFormat::parse("CSV").unwrap(), ImportFormat::Csv);
        assert_eq!(ImportFormat::parse("jsonl").unwrap(), ImportFormat::Jsonl);
        assert!(ImportFormat::parse("yaml").is_err());
        assert_eq!(ImportTarget::parse("tag").unwrap(), ImportTarget::Vertex);
        assert_eq!(ImportTarget::parse("vertex").unwrap(), ImportTarget::Vertex);
        assert_eq!(ImportTarget::parse("edge").unwrap(), ImportTarget::Edge);
        assert!(ImportTarget::parse("relation").is_err());
    }

    #[test]
    fn parse_csv_vertex_statements() {
        let csv = "vid,name,age\n1,Alice,30\n2,Bob,25\n";
        let parsed = parse(
            ImportFormat::Csv,
            ImportTarget::Vertex,
            "Person",
            csv.as_bytes(),
        );
        assert_eq!(parsed.rows.len(), 2);
        assert!(parsed.failures.is_empty());
        assert_eq!(parsed.rows[0].line, 2);
        assert_eq!(
            parsed.rows[0].statement,
            "INSERT VERTEX Person (name, age) VALUES \"1\":(\"Alice\", \"30\")"
        );
        assert_eq!(
            parsed.rows[1].statement,
            "INSERT VERTEX Person (name, age) VALUES \"2\":(\"Bob\", \"25\")"
        );
    }

    #[test]
    fn parse_csv_edge_statements() {
        let csv = "src,dst,weight\n1,2,0.5\n3,4,1.5\n";
        let parsed = parse(
            ImportFormat::Csv,
            ImportTarget::Edge,
            "Follows",
            csv.as_bytes(),
        );
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(
            parsed.rows[0].statement,
            "INSERT EDGE Follows (weight) VALUES \"1\"->\"2\":(\"0.5\")"
        );
        assert_eq!(parsed.rows[1].line, 3);
    }

    #[test]
    fn parse_json_array_vertex_statements() {
        let json = r#"[{"_id":"v1","name":"Alice","age":30},{"_id":"v2","name":"Bob","age":25}]"#;
        let parsed = parse(
            ImportFormat::Json,
            ImportTarget::Vertex,
            "Person",
            json.as_bytes(),
        );
        assert_eq!(parsed.rows.len(), 2);
        // serde_json::Map iterates keys in sorted order.
        assert_eq!(
            parsed.rows[0].statement,
            "INSERT VERTEX Person (age, name) VALUES \"v1\":(30, \"Alice\")"
        );
    }

    #[test]
    fn parse_jsonl_edge_statements() {
        let jsonl = "{\"_src\":\"1\",\"_dst\":\"2\",\"weight\":0.5}\n{\"_src\":\"3\",\"_dst\":\"4\",\"weight\":1.5}\n";
        let parsed = parse(
            ImportFormat::Jsonl,
            ImportTarget::Edge,
            "Follows",
            jsonl.as_bytes(),
        );
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(
            parsed.rows[0].statement,
            "INSERT EDGE Follows (weight) VALUES \"1\"->\"2\":(0.5)"
        );
        assert_eq!(parsed.rows[1].line, 2);
    }

    #[test]
    fn parse_escapes_quotes_in_values() {
        let csv = "vid,name\n1,\"Al\"\"ice\"\n";
        let parsed = parse(
            ImportFormat::Csv,
            ImportTarget::Vertex,
            "Person",
            csv.as_bytes(),
        );
        assert_eq!(parsed.rows.len(), 1);
        assert!(
            parsed.rows[0].statement.contains("Al\\\"ice"),
            "{}",
            parsed.rows[0].statement
        );
    }

    #[test]
    fn parse_rejects_unsupported_format() {
        assert!(ImportFormat::parse("yaml").is_err());
    }

    #[test]
    fn parse_rejects_json_edge_without_src() {
        let json = r#"[{"_dst":"2"}]"#;
        let parsed = parse(
            ImportFormat::Json,
            ImportTarget::Edge,
            "Follows",
            json.as_bytes(),
        );
        assert!(parsed.rows.is_empty());
        assert_eq!(parsed.failures.len(), 1);
        assert!(parsed.failures[0].message.contains("_src"));
    }

    #[test]
    fn parse_empty_csv_yields_no_statements() {
        let parsed = parse(
            ImportFormat::Csv,
            ImportTarget::Vertex,
            "Person",
            b"vid,name\n",
        );
        assert!(parsed.is_empty());
    }

    #[test]
    fn malformed_rows_are_counted_with_line_numbers() {
        let jsonl = concat!(
            "{\"_src\":\"1\",\"_dst\":\"2\",\"weight\":1}\n",
            "not json\n",
            "{\"_dst\":\"4\",\"weight\":2}\n",
            "{\"_src\":\"5\",\"_dst\":\"6\",\"weight\":3}\n",
        );
        let parsed = parse(
            ImportFormat::Jsonl,
            ImportTarget::Edge,
            "Follows",
            jsonl.as_bytes(),
        );
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].line, 1);
        assert_eq!(parsed.rows[1].line, 4);
        assert_eq!(parsed.failures.len(), 2);
        assert_eq!(parsed.failures[0].line, 2);
        assert!(parsed.failures[0].message.contains("line 2"));
        assert_eq!(parsed.failures[1].line, 3);
        assert!(parsed.failures[1].message.contains("line 3"));
        assert!(parsed.failures[1].message.contains("_src"));
    }

    #[test]
    fn json_array_non_object_items_are_counted() {
        let json = r#"[{"_id":"v1","name":"Alice"},42]"#;
        let parsed = parse(
            ImportFormat::Json,
            ImportTarget::Vertex,
            "Person",
            json.as_bytes(),
        );
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.failures.len(), 1);
        assert!(parsed.failures[0].message.contains("must be objects"));
    }
}
