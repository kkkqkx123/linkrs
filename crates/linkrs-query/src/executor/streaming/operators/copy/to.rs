use std::fs::File;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::executor::streaming::operators::spec::CopyTarget;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
use linkrs_core::Value;

pub fn execute_copy_to(
    storage_lock: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    target: &CopyTarget,
    file_path: &str,
    header: bool,
    delimiter: u8,
) -> Result<u64, QueryError> {
    let delim = char::from(delimiter);
    let file = File::create(file_path)
        .map_err(|e| QueryError::execution(format!("COPY TO: cannot create '{file_path}': {e}")))?;
    let mut writer = std::io::BufWriter::new(file);

    match target {
        CopyTarget::Vertex(tag) => {
            let prop_names: Vec<String> = {
                let read = storage_lock.read();
                match read.get_tag(space_name, tag) {
                    Ok(Some(info)) => info.properties.iter().map(|p| p.name.clone()).collect(),
                    Ok(None) => {
                        return Err(QueryError::execution(format!("Tag '{tag}' not found")))
                    }
                    Err(e) => return Err(QueryError::execution(e.to_string())),
                }
            };
            let vertices = {
                let read = storage_lock.read();
                read.scan_vertices_by_tag(space_name, tag)
                    .map_err(|e| QueryError::execution(e.to_string()))?
            };
            if header {
                let mut head = vec!["vid".to_string()];
                head.extend(prop_names.iter().cloned());
                write_csv_line(&mut writer, &head, delim)?;
            }
            let mut count = 0u64;
            for vertex in &vertices {
                let props = vertex.properties();
                let mut cells = vec![csv_cell(&Value::from(vertex.vid), delim)];
                cells.extend(prop_names.iter().map(|name| {
                    props
                        .get(name.as_str())
                        .map_or_else(String::new, |v| csv_cell(v, delim))
                }));
                write_csv_line(&mut writer, &cells, delim)?;
                count += 1;
            }
            Ok(count)
        }
        CopyTarget::Edge(edge_type) => {
            let prop_names: Vec<String> = {
                let read = storage_lock.read();
                match read.get_edge_type(space_name, edge_type) {
                    Ok(Some(info)) => info.properties.iter().map(|p| p.name.clone()).collect(),
                    Ok(None) => {
                        return Err(QueryError::execution(format!(
                            "Edge type '{edge_type}' not found"
                        )))
                    }
                    Err(e) => return Err(QueryError::execution(e.to_string())),
                }
            };
            let edges = {
                let read = storage_lock.read();
                read.scan_edges_by_type(space_name, edge_type)
                    .map_err(|e| QueryError::execution(e.to_string()))?
            };
            if header {
                let mut head = vec!["src".to_string(), "dst".to_string()];
                head.extend(prop_names.iter().cloned());
                write_csv_line(&mut writer, &head, delim)?;
            }
            let mut count = 0u64;
            for edge in &edges {
                let mut cells = vec![
                    csv_cell(&Value::from(*edge.src()), delim),
                    csv_cell(&Value::from(*edge.dst()), delim),
                ];
                cells.extend(prop_names.iter().map(|name| {
                    edge.get_property(name.as_str())
                        .map_or_else(String::new, |v| csv_cell(v, delim))
                }));
                write_csv_line(&mut writer, &cells, delim)?;
                count += 1;
            }
            Ok(count)
        }
    }
}

/// Format one value as a CSV cell: empty for NULL, quoted when the rendered
/// text contains the delimiter, a quote, or a newline.
fn csv_cell(value: &Value, delimiter: char) -> String {
    let text = match value {
        Value::Null(_) => String::new(),
        Value::String(s) => s.to_string(),
        Value::FixedString(s) => s.to_string(),
        other => format!("{other}"),
    };
    if text.contains(delimiter) || text.contains('"') || text.contains('\n') || text.contains('\r')
    {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn write_csv_line<W: std::io::Write>(
    writer: &mut W,
    cells: &[String],
    delimiter: char,
) -> Result<(), QueryError> {
    let mut line = String::with_capacity(32 * cells.len());
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            line.push(delimiter);
        }
        line.push_str(cell);
    }
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .map_err(|e| QueryError::execution(format!("COPY TO: write failed: {e}")))
}
