use super::{StorageSchemaOps, StorageWriter};
use graphdb_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo};
use graphdb_core::{StorageError, Value};
use std::io::BufRead;
use std::path::Path;

/// Format a `Value` for CSV export (handles commas and quotes).
pub(crate) fn format_csv_value(v: &Value) -> String {
    match v {
        Value::Null(_) => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::SmallInt(i) => i.to_string(),
        Value::Int(i) => i.to_string(),
        Value::BigInt(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(f) => f.to_string(),
        Value::String(s) => {
            let s: &str = s.as_ref();
            if s.contains(',') || s.contains('"') || s.contains('\n') {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        }
        other => format!("{}", other),
    }
}

/// Parse a type name string into a `DataType`.
pub(crate) fn parse_data_type(s: &str) -> graphdb_core::types::DataType {
    let upper = s.trim().to_uppercase();
    match upper.as_str() {
        "INT8" | "TINYINT" => graphdb_core::types::DataType::SmallInt,
        "INT16" | "SMALLINT" => graphdb_core::types::DataType::SmallInt,
        "INT32" | "INT" | "INTEGER" => graphdb_core::types::DataType::Int,
        "INT64" | "BIGINT" => graphdb_core::types::DataType::BigInt,
        "FLOAT" | "FLOAT32" => graphdb_core::types::DataType::Float,
        "DOUBLE" | "FLOAT64" => graphdb_core::types::DataType::Double,
        "BOOL" | "BOOLEAN" => graphdb_core::types::DataType::Bool,
        "STRING" | "TEXT" | "VARCHAR" => graphdb_core::types::DataType::String,
        _ => graphdb_core::types::DataType::String,
    }
}

/// Import a space from CSV files under `path/<space_name>/`.
///
/// Expects a `schema.json` metadata file and `<tag>.csv` / `<edge_type>.csv` data files.
pub(crate) fn import_space_impl<S: StorageWriter + StorageSchemaOps + ?Sized>(
    storage: &mut S,
    space: &str,
    path: &Path,
) -> Result<(), StorageError> {
    let base = path.join(space);
    if !base.exists() {
        return Err(StorageError::not_found(format!(
            "Import directory '{}' does not exist",
            base.display()
        )));
    }

    let schema_path = base.join("schema.json");
    let schema_content = if schema_path.exists() {
        std::fs::read_to_string(&schema_path)
            .map_err(|e| StorageError::io_error(format!("Failed to read schema.json: {e}")))?
    } else {
        return Err(StorageError::not_found(
            "schema.json not found in import directory".to_string(),
        ));
    };

    let schema_meta: serde_json::Value = serde_json::from_str(&schema_content)
        .map_err(|e| StorageError::parse_error(format!("Invalid schema.json: {e}")))?;

    let vid_type = graphdb_core::types::DataType::String;
    let mut space_info = SpaceInfo::new(space.to_string()).with_vid_type(vid_type);
    let _ = StorageSchemaOps::create_space(storage, &mut space_info);

    if let Some(tags) = schema_meta.get("tags").and_then(|t| t.as_array()) {
        for tag_meta in tags {
            let tag_name = tag_meta
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if tag_name.is_empty() {
                continue;
            }

            let mut tag_info = TagInfo::new(tag_name.to_string());
            if let Some(props) = tag_meta.get("properties").and_then(|p| p.as_array()) {
                for prop in props {
                    let prop_name = prop.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let prop_type = prop.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                    let data_type = parse_data_type(prop_type);
                    tag_info
                        .properties
                        .push(PropertyDef::new(prop_name.to_string(), data_type));
                }
            }
            let _ = StorageSchemaOps::create_tag(storage, space, &tag_info);

            let csv_path = base.join(format!("{tag_name}.csv"));
            if csv_path.exists() {
                import_vertex_csv_from_path(space, tag_name, &csv_path, storage)?;
            }
        }
    }

    if let Some(edge_types) = schema_meta.get("edge_types").and_then(|t| t.as_array()) {
        for et_meta in edge_types {
            let et_name = et_meta.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if et_name.is_empty() {
                continue;
            }

            let mut et_info = EdgeTypeInfo::new(et_name.to_string());
            et_info.src_tag_name = et_meta
                .get("src_tag")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            et_info.dst_tag_name = et_meta
                .get("dst_tag")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(props) = et_meta.get("properties").and_then(|p| p.as_array()) {
                for prop in props {
                    let prop_name = prop.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let prop_type = prop.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                    let data_type = parse_data_type(prop_type);
                    et_info
                        .properties
                        .push(PropertyDef::new(prop_name.to_string(), data_type));
                }
            }
            let _ = StorageSchemaOps::create_edge_type(storage, space, &et_info);

            let csv_path = base.join(format!("{et_name}.csv"));
            if csv_path.exists() {
                import_edge_csv_from_path(space, et_name, &csv_path, storage)?;
            }
        }
    }

    Ok(())
}

/// Import vertex data from a CSV file into the given space and tag.
pub(crate) fn import_vertex_csv_from_path<W: StorageWriter + ?Sized>(
    space: &str,
    tag_name: &str,
    csv_path: &Path,
    writer: &mut W,
) -> Result<(), StorageError> {
    let file = std::fs::File::open(csv_path)
        .map_err(|e| StorageError::io_error(format!("Failed to open {}: {e}", csv_path.display())))?;
    let reader = std::io::BufReader::new(file);
    let mut lines = reader.lines();

    // Read header
    let header_line = match lines.next() {
        Some(Ok(line)) => line,
        _ => return Ok(()),
    };
    let headers: Vec<String> = header_line
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .collect();

    // Find vid and id column indices
    let vid_idx = headers.iter().position(|h| h.as_str() == "vid");
    let id_idx = headers.iter().position(|h| h.as_str() == "id");

    // Property columns (everything that's not vid or id)
    let prop_cols: Vec<(usize, String)> = headers
        .iter()
        .enumerate()
        .filter(|(_, h)| h.as_str() != "vid" && h.as_str() != "id")
        .map(|(i, h)| (i, h.clone()))
        .collect();

    let mut vertices = Vec::new();
    for line in lines {
        let line = line.map_err(|e| StorageError::io_error(format!("CSV read error: {e}")))?;
        let fields: Vec<&str> = line.split(',').collect();

        let vid = vid_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);
        let id = id_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(vid);

        let mut properties = std::collections::HashMap::new();
        for (col_idx, col_name) in &prop_cols {
            if let Some(val_str) = fields.get(*col_idx) {
                let val_str = val_str.trim().trim_matches('"');
                if !val_str.is_empty() {
                    properties.insert(
                        col_name.clone(),
                        graphdb_core::Value::string(val_str),
                    );
                }
            }
        }

        let vertex = graphdb_core::Vertex {
            vid: graphdb_core::types::VertexId::from_int64(id),
            id,
            tags: vec![graphdb_core::Tag::new(tag_name.to_string(), std::collections::HashMap::new())],
            properties,
        };
        vertices.push(vertex);

        if vertices.len() >= 1000 {
            writer.batch_insert_vertices(space, vertices.clone())?;
            vertices.clear();
        }
    }

    if !vertices.is_empty() {
        writer.batch_insert_vertices(space, vertices)?;
    }

    Ok(())
}

/// Import edge data from a CSV file into the given space and edge type.
pub(crate) fn import_edge_csv_from_path<W: StorageWriter + ?Sized>(
    space: &str,
    edge_type: &str,
    csv_path: &Path,
    writer: &mut W,
) -> Result<(), StorageError> {
    let file = std::fs::File::open(csv_path)
        .map_err(|e| StorageError::io_error(format!("Failed to open {}: {e}", csv_path.display())))?;
    let reader = std::io::BufReader::new(file);
    let mut lines = reader.lines();

    // Read header
    let header_line = match lines.next() {
        Some(Ok(line)) => line,
        _ => return Ok(()),
    };
    let headers: Vec<String> = header_line
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .collect();

    let src_idx = headers.iter().position(|h| h.as_str() == "src");
    let dst_idx = headers.iter().position(|h| h.as_str() == "dst");
    let rank_idx = headers.iter().position(|h| h.as_str() == "ranking");

    let prop_cols: Vec<(usize, String)> = headers
        .iter()
        .enumerate()
        .filter(|(_, h)| h.as_str() != "src" && h.as_str() != "dst" && h.as_str() != "ranking")
        .map(|(i, h)| (i, h.clone()))
        .collect();

    let mut edges = Vec::new();
    for line in lines {
        let line = line.map_err(|e| StorageError::io_error(format!("CSV read error: {e}")))?;
        let fields: Vec<&str> = line.split(',').collect();

        let src = src_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);
        let dst = dst_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);
        let ranking = rank_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().trim_matches('"').parse::<i64>().ok())
            .unwrap_or(0);

        let mut props = std::collections::HashMap::new();
        for (col_idx, col_name) in &prop_cols {
            if let Some(val_str) = fields.get(*col_idx) {
                let val_str = val_str.trim().trim_matches('"');
                if !val_str.is_empty() {
                    props.insert(
                        col_name.clone(),
                        graphdb_core::Value::string(val_str),
                    );
                }
            }
        }

        let edge = graphdb_core::Edge {
            src: graphdb_core::types::VertexId::from_int64(src),
            dst: graphdb_core::types::VertexId::from_int64(dst),
            edge_type: edge_type.to_string(),
            ranking,
            props,
        };
        edges.push(edge);

        if edges.len() >= 1000 {
            writer.batch_insert_edges(space, edges.clone())?;
            edges.clear();
        }
    }

    if !edges.is_empty() {
        writer.batch_insert_edges(space, edges)?;
    }

    Ok(())
}
