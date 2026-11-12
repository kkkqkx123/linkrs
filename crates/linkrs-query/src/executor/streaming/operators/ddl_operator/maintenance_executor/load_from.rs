use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use linkrs_core::error::QueryError;
use linkrs_core::Value;

use super::common::{column_names_from_width, columns_for, json_to_value};

pub(crate) fn execute_load_from(
    op: &mut super::super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::super::DdlOperatorKind::LoadFrom {
        source_kind,
        source_value,
        func_name,
        func_args_json,
        options,
        col_names,
        headers,
        emitted,
        ..
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    if source_kind == "query" {
        return Err(QueryError::execution(
            "LOAD FROM (query) must be expanded at plan time".to_string(),
        ));
    }

    if source_kind == "table_func" {
        let name = func_name.clone().unwrap_or_default();
        let args_json_str = func_args_json.clone().unwrap_or_else(|| "[]".to_string());

        let arg_values: Vec<serde_json::Value> =
            serde_json::from_str(&args_json_str).map_err(|e| {
                QueryError::execution(format!("LOAD FROM table function args parse error: {e}"))
            })?;
        let func_args: Vec<Value> = arg_values.iter().map(json_to_value).collect();

        let registry = crate::executor::expression::functions::registry::global_registry();
        let registry = registry.read();

        if !registry.contains_table_function(&name) {
            return Err(QueryError::execution(format!(
                "LOAD FROM unknown table function '{name}'"
            )));
        }

        let rows = registry
            .execute_table_function(&name, &func_args)
            .map_err(|e| {
                QueryError::execution(format!("LOAD FROM table function '{name}' failed: {e}"))
            })?;

        if rows.is_empty() {
            let schema = Arc::new(Schema::new(vec![]));
            return Ok(Some(DataChunk::new(vec![], schema)));
        }

        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if !headers.is_empty() && headers.len() != num_cols {
            return Err(QueryError::execution(format!(
                "LOAD WITH HEADERS declares {} columns but table function yields {}",
                headers.len(),
                num_cols
            )));
        }
        let schema_names: Vec<String> = if !headers.is_empty() {
            headers.clone()
        } else if col_names.len() == num_cols {
            col_names.clone()
        } else {
            (0..num_cols).map(|i| format!("col{i}")).collect()
        };
        let schema_cols: Vec<ColumnInfo> = schema_names
            .iter()
            .map(|name| ColumnInfo {
                name: name.clone(),
                data_type: "string".to_string(),
            })
            .collect();
        let schema = Arc::new(Schema::new(schema_cols));

        return Ok(Some(DataChunk::new(rows, schema)));
    }

    if source_kind == "glob" {
        let pattern = source_value.clone();
        let mut matched_paths: Vec<std::path::PathBuf> = glob::glob(&pattern)
            .map_err(|e| QueryError::execution(format!("LOAD FROM glob pattern error: {e}")))?
            .filter_map(|entry| entry.ok())
            .filter(|path| path.is_file())
            .collect();
        matched_paths.sort();

        if matched_paths.is_empty() {
            return Err(QueryError::execution(format!(
                "LOAD FROM glob '{pattern}' matched no files"
            )));
        }

        let header = options
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("header"))
            .map(|(_, v)| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);
        let delimiter = options
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("delimiter"))
            .and_then(|(_, v)| v.as_bytes().first().copied())
            .unwrap_or(b',');

        let mut all_rows: Vec<Vec<Value>> = Vec::new();
        let mut schema_names: Option<Vec<String>> = None;

        for file_path in &matched_paths {
            let file = std::fs::File::open(file_path).map_err(|e| {
                QueryError::execution(format!(
                    "LOAD FROM glob failed to open '{}': {e}",
                    file_path.display()
                ))
            })?;
            let mut reader = csv::ReaderBuilder::new()
                .has_headers(header)
                .delimiter(delimiter)
                .trim(csv::Trim::All)
                .flexible(true)
                .from_reader(std::io::BufReader::new(file));

            if schema_names.is_none() {
                let names: Vec<String> = if !headers.is_empty() {
                    headers.clone()
                } else if header {
                    reader
                        .headers()
                        .map_err(|e| {
                            QueryError::execution(format!("LOAD FROM glob header error: {e}"))
                        })?
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                } else {
                    col_names.clone()
                };
                schema_names = Some(names);
            }

            for result in reader.records() {
                let record = result
                    .map_err(|e| QueryError::execution(format!("LOAD FROM glob row error: {e}")))?;
                if !headers.is_empty() && record.len() != headers.len() {
                    return Err(QueryError::execution(format!(
                        "LOAD WITH HEADERS declares {} columns but '{}' yields {}",
                        headers.len(),
                        file_path.display(),
                        record.len()
                    )));
                }
                let row: Vec<Value> = record.iter().map(|s| Value::String(s.into())).collect();
                all_rows.push(row);
            }
        }

        let schema_names = match schema_names {
            Some(names) if !names.is_empty() => names,
            _ => column_names_from_width(all_rows.first().map(Vec::len).unwrap_or(0)),
        };
        let schema = Arc::new(Schema::new(columns_for(&schema_names)));
        return Ok(Some(DataChunk::new(all_rows, schema)));
    }

    if source_kind != "file" {
        return Err(QueryError::execution(format!(
            "LOAD FROM source kind '{source_kind}' is not yet supported"
        )));
    }

    let header = options
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("header"))
        .map(|(_, v)| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(true);
    let delimiter = options
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("delimiter"))
        .and_then(|(_, v)| v.as_bytes().first().copied())
        .unwrap_or(b',');

    let file_path = source_value.clone();
    let file = std::fs::File::open(&file_path).map_err(|e| {
        QueryError::execution(format!("LOAD FROM failed to open '{file_path}': {e}"))
    })?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(header)
        .delimiter(delimiter)
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(std::io::BufReader::new(file));

    let declared_names: Vec<String> = if !headers.is_empty() {
        headers.clone()
    } else if header {
        reader
            .headers()
            .map_err(|e| QueryError::execution(format!("LOAD FROM header error: {e}")))?
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        col_names.clone()
    };

    let mut rows: Vec<Vec<Value>> = Vec::new();
    for result in reader.records() {
        let record =
            result.map_err(|e| QueryError::execution(format!("LOAD FROM row error: {e}")))?;
        if !headers.is_empty() && record.len() != headers.len() {
            return Err(QueryError::execution(format!(
                "LOAD WITH HEADERS declares {} columns but '{file_path}' yields {}",
                headers.len(),
                record.len()
            )));
        }
        let row: Vec<Value> = record.iter().map(|s| Value::String(s.into())).collect();
        rows.push(row);
    }

    let schema_names = if declared_names.is_empty() {
        column_names_from_width(rows.first().map(Vec::len).unwrap_or(0))
    } else {
        declared_names
    };
    let schema = Arc::new(Schema::new(columns_for(&schema_names)));
    Ok(Some(DataChunk::new(rows, schema)))
}
