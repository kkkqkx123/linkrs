//! COPY FROM execution: parallel CSV import into vertices or edges.
//!
//! Pipeline: read CSV records → map columns (header names or positional) →
//! parse values in parallel per batch (`rayon`) → batch-insert through the
//! storage writer. Column mapping is computed once up front so the
//! per-record hot path is a direct index lookup.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use parking_lot::RwLock;
use rayon::prelude::*;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::vertex_edge_path::{Edge, Tag, Vertex};
use crate::core::Value;
use crate::executor::streaming::operators::spec::CopyTarget;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::storage::{QueryStorage, StorageWriter};

/// Default batch size when the statement does not specify one.
const DEFAULT_BATCH_SIZE: usize = 1000;

/// Execute a COPY FROM statement and return the number of rows inserted.
#[allow(clippy::too_many_arguments)]
pub fn execute_copy_from(
    storage_lock: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    target: &CopyTarget,
    file_path: &str,
    header: bool,
    delimiter: u8,
    batch_size: usize,
    runtime: Option<Arc<ExecutionRuntime>>,
) -> Result<u64, QueryError> {
    let batch_sz = if batch_size == 0 {
        DEFAULT_BATCH_SIZE
    } else {
        batch_size
    };

    match target {
        CopyTarget::Vertex(tag) => {
            let schema_props = schema_property_names_vertex(storage_lock, space_name, tag, header)?;
            run_import(
                storage_lock,
                space_name,
                file_path,
                header,
                delimiter,
                batch_sz,
                runtime,
                &ImportPlan::Vertices {
                    tag: tag.clone(),
                    schema_props,
                },
            )
        }
        CopyTarget::Edge(edge_type) => {
            let schema_props =
                schema_property_names_edge(storage_lock, space_name, edge_type, header)?;
            run_import(
                storage_lock,
                space_name,
                file_path,
                header,
                delimiter,
                batch_sz,
                runtime,
                &ImportPlan::Edges {
                    edge_type: edge_type.clone(),
                    schema_props,
                },
            )
        }
    }
}

/// Property names to assign when the CSV has no header row: taken from the
/// tag / edge-type schema in declaration order.
fn schema_property_names_vertex(
    storage_lock: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    tag: &str,
    header: bool,
) -> Result<Option<Vec<String>>, QueryError> {
    if header {
        return Ok(None);
    }
    let read = storage_lock.read();
    match read.get_tag(space_name, tag) {
        Ok(Some(info)) => Ok(Some(
            info.properties.iter().map(|p| p.name.clone()).collect(),
        )),
        Ok(None) => Err(QueryError::execution(format!("Tag '{tag}' not found"))),
        Err(e) => Err(QueryError::execution(e.to_string())),
    }
}

fn schema_property_names_edge(
    storage_lock: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    edge_type: &str,
    header: bool,
) -> Result<Option<Vec<String>>, QueryError> {
    if header {
        return Ok(None);
    }
    let read = storage_lock.read();
    match read.get_edge_type(space_name, edge_type) {
        Ok(Some(info)) => Ok(Some(
            info.properties.iter().map(|p| p.name.clone()).collect(),
        )),
        Ok(None) => Err(QueryError::execution(format!(
            "Edge type '{edge_type}' not found"
        ))),
        Err(e) => Err(QueryError::execution(e.to_string())),
    }
}

/// Fully resolved import plan: how to turn each CSV record into an entity.
enum ImportPlan {
    Vertices {
        tag: String,
        /// Schema property names for the no-header case.
        schema_props: Option<Vec<String>>,
    },
    Edges {
        edge_type: String,
        schema_props: Option<Vec<String>>,
    },
}

#[allow(clippy::too_many_arguments)]
fn run_import(
    storage_lock: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    file_path: &str,
    header: bool,
    delimiter: u8,
    batch_size: usize,
    runtime: Option<Arc<ExecutionRuntime>>,
    plan: &ImportPlan,
) -> Result<u64, QueryError> {
    let mut source = CsvSource::open(file_path, header, delimiter)?;

    // Resolve the column layout once: key column(s) plus (record index,
    // property name) pairs for every property column.
    let mapping = match plan {
        ImportPlan::Vertices { schema_props, .. } => build_column_mapping(
            source.headers(),
            header,
            schema_props.as_deref(),
            KeyLayout::Vertex,
        )?,
        ImportPlan::Edges { schema_props, .. } => build_column_mapping(
            source.headers(),
            header,
            schema_props.as_deref(),
            KeyLayout::Edge,
        )?,
    };

    // All batches share one auto-commit group window: a single write gate,
    // a shared undo log, and one commit point at the end, so a failure in
    // any batch rolls back the whole import.
    // The group window and bound writers use interior locking, so a read
    // guard is sufficient for the whole import.
    let guard = storage_lock.read();
    let storage: &dyn QueryStorage = &*guard;
    let window = storage.begin_auto_commit_group().map_err(|e| {
        QueryError::execution(format!("COPY FROM failed to open write window: {e}"))
    })?;

    let result = (|| -> Result<u64, QueryError> {
        let mut total: u64 = 0;
        let mut skipped = 0u64;

        while !source.is_eof() {
            if let Some(rt) = &runtime {
                rt.ensure_not_cancelled()
                    .map_err(|e| QueryError::execution(e.to_string()))?;
            }
            let records = source.next_batch(batch_size, &mapping, &mut skipped)?;
            if records.is_empty() {
                continue;
            }

            let inserted = {
                let mut writer = storage.bind_auto_commit_writer(&window).map_err(|e| {
                    QueryError::execution(format!("COPY FROM failed to bind writer: {e}"))
                })?;
                match plan {
                    ImportPlan::Vertices { tag, .. } => {
                        flush_vertices(&mut *writer, space_name, tag, &mapping, &records)?
                    }
                    ImportPlan::Edges { edge_type, .. } => {
                        flush_edges(&mut *writer, space_name, edge_type, &mapping, &records)?
                    }
                }
            };
            total += inserted;
        }

        if skipped > 0 {
            log::warn!(
                "COPY FROM '{file_path}': skipped {skipped} malformed record(s) \
                 (wrong field count)"
            );
        }
        Ok(total)
    })();

    match result {
        Ok(total) => {
            storage
                .finalize_auto_commit_group(&window)
                .map_err(|e| QueryError::execution(format!("COPY FROM commit failed: {e}")))?;
            Ok(total)
        }
        Err(error) => {
            if let Err(rollback_error) = storage.rollback_auto_commit_group(&window) {
                log::error!("COPY FROM rollback failed: {rollback_error}");
            }
            Err(error)
        }
    }
}

/// Which key column(s) the record carries before its property columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyLayout {
    /// `vid` plus property columns.
    Vertex,
    /// `src` + `dst` plus property columns.
    Edge,
}

fn flush_vertices(
    writer: &mut dyn StorageWriter,
    space_name: &str,
    tag: &str,
    mapping: &ColumnMapping,
    records: &[Vec<String>],
) -> Result<u64, QueryError> {
    let vertices: Vec<Vertex> = records
        .par_iter()
        .map(|rec| {
            let vid_str = rec[mapping.vid_index].trim();
            let vid = parse_vid(vid_str)?;
            let props = collect_properties(mapping, rec);
            Ok(Vertex::new(vid, vec![Tag::new(tag.to_string(), props)]))
        })
        .collect::<Result<Vec<_>, QueryError>>()?;
    let count = vertices.len() as u64;
    StorageWriter::batch_insert_vertices(writer, space_name, vertices)
        .map_err(|e| QueryError::execution(e.to_string()))?;
    Ok(count)
}

fn flush_edges(
    writer: &mut dyn StorageWriter,
    space_name: &str,
    edge_type: &str,
    mapping: &ColumnMapping,
    records: &[Vec<String>],
) -> Result<u64, QueryError> {
    let edges: Vec<Edge> = records
        .par_iter()
        .map(|rec| {
            let src = parse_vid(rec[mapping.src_index].trim())?;
            let dst = parse_vid(rec[mapping.dst_index].trim())?;
            let props = collect_properties(mapping, rec);
            Ok(Edge {
                src,
                dst,
                edge_type: edge_type.to_string(),
                ranking: 0,
                props,
            })
        })
        .collect::<Result<Vec<_>, QueryError>>()?;
    let count = edges.len() as u64;
    StorageWriter::batch_insert_edges(writer, space_name, edges)
        .map_err(|e| QueryError::execution(e.to_string()))?;
    Ok(count)
}

fn collect_properties(mapping: &ColumnMapping, rec: &[String]) -> HashMap<String, Value> {
    let mut props = HashMap::with_capacity(mapping.property_indices.len());
    for &(idx, ref name) in &mapping.property_indices {
        props.insert(name.clone(), parse_copy_value(rec[idx].trim()));
    }
    props
}

/// Parse a vertex id from a COPY cell. Numeric cells map to the integer id
/// domain; anything else becomes a string id. Empty cells are an error.
fn parse_vid(s: &str) -> Result<VertexId, QueryError> {
    if s.is_empty() {
        return Err(QueryError::execution(
            "COPY FROM: empty vertex id cell".to_string(),
        ));
    }
    if let Ok(i) = s.parse::<i64>() {
        Ok(VertexId::from_int64(i))
    } else {
        Ok(VertexId::from_string(s))
    }
}

// ---------------------------------------------------------------------------
// CSV source and column mapping
// ---------------------------------------------------------------------------

/// Column layout resolved once per import: where the key column(s) live and
/// which record indices map to which property names.
#[derive(Debug, Clone)]
struct ColumnMapping {
    /// Record index of the vertex id (vertex imports).
    vid_index: usize,
    /// Record index of the source vertex id (edge imports).
    src_index: usize,
    /// Record index of the destination vertex id (edge imports).
    dst_index: usize,
    /// `(record index, property name)` for every property column, in order.
    property_indices: Vec<(usize, String)>,
    /// Field count a well-formed record must have.
    expected_width: usize,
}

/// Locate a column by case-insensitive name among several aliases.
fn find_column(headers: &[String], aliases: &[&str]) -> Option<usize> {
    headers
        .iter()
        .position(|h| aliases.iter().any(|a| h.eq_ignore_ascii_case(a)))
}

/// Resolve the column mapping for one import.
///
/// Header imports locate keys by name (`vid`/`id`/`_id`/`vertex_id`,
/// `src`/`_src`/`source`, `dst`/`_dst`/`destination`/`dest`) and treat every
/// remaining column as a property named after its header. Positional imports
/// (no header) use the first column(s) as keys and assign schema property
/// names in declaration order.
fn build_column_mapping(
    headers: &[String],
    has_header: bool,
    schema_props: Option<&[String]>,
    layout: KeyLayout,
) -> Result<ColumnMapping, QueryError> {
    if !has_header {
        // Positional layout: key column(s) first, then properties in schema order.
        let schema_props = schema_props.ok_or_else(|| {
            QueryError::execution(
                "COPY FROM without HEADER requires the target tag / edge type to \
                 declare properties"
                    .to_string(),
            )
        })?;
        let key_count = match layout {
            KeyLayout::Vertex => 1usize,
            KeyLayout::Edge => 2,
        };
        let property_indices: Vec<(usize, String)> = schema_props
            .iter()
            .enumerate()
            .map(|(i, name)| (key_count + i, name.clone()))
            .collect();
        return Ok(ColumnMapping {
            vid_index: 0,
            src_index: 0,
            dst_index: 1,
            property_indices,
            expected_width: key_count + schema_props.len(),
        });
    }

    match layout {
        KeyLayout::Vertex => {
            let vid_index =
                find_column(headers, &["vid", "id", "_id", "vertex_id"]).ok_or_else(|| {
                    QueryError::execution(
                        "COPY FROM: header row has no vertex id column \
                         (expected one of: vid, id, _id, vertex_id)"
                            .to_string(),
                    )
                })?;
            let property_indices = excluded_pairs(headers, &[vid_index]);
            Ok(ColumnMapping {
                vid_index,
                src_index: vid_index,
                dst_index: vid_index,
                property_indices,
                expected_width: headers.len(),
            })
        }
        KeyLayout::Edge => {
            let src_hit = find_column(headers, &["src", "_src", "source"]);
            let dst_hit = find_column(headers, &["dst", "_dst", "destination", "dest"]);
            let (src_index, dst_index) = match (src_hit, dst_hit) {
                (Some(s), Some(d)) => {
                    if s == d {
                        return Err(QueryError::execution(
                            "COPY FROM: src and dst resolve to the same column".to_string(),
                        ));
                    }
                    (s, d)
                }
                // Neither named: fall back to the conventional leading pair.
                (None, None) => (0, 1),
                // Exactly one named: refusing to guess prevents silently
                // binding an endpoint to an unrelated property column.
                (Some(_), None) => {
                    return Err(QueryError::execution(
                        "COPY FROM: header row names a source column but no destination \
                         column (expected one of: dst, _dst, destination, dest)"
                            .to_string(),
                    ))
                }
                (None, Some(_)) => {
                    return Err(QueryError::execution(
                        "COPY FROM: header row names a destination column but no source \
                         column (expected one of: src, _src, source)"
                            .to_string(),
                    ))
                }
            };
            let property_indices = excluded_pairs(headers, &[src_index, dst_index]);
            Ok(ColumnMapping {
                vid_index: src_index,
                src_index,
                dst_index,
                property_indices,
                expected_width: headers.len(),
            })
        }
    }
}

/// `(index, name)` pairs for every column except the excluded key columns.
fn excluded_pairs(headers: &[String], exclude: &[usize]) -> Vec<(usize, String)> {
    headers
        .iter()
        .enumerate()
        .filter(|(i, _)| !exclude.contains(i))
        .map(|(i, h)| (i, h.clone()))
        .collect()
}

/// Buffered CSV record source with batch reads.
struct CsvSource {
    headers: Vec<String>,
    reader: csv::StringRecordsIntoIter<std::io::BufReader<File>>,
    finished: bool,
}

impl CsvSource {
    fn open(file_path: &str, header: bool, delimiter: u8) -> Result<Self, QueryError> {
        let file = File::open(file_path).map_err(|e| {
            QueryError::execution(format!("COPY FROM failed to open '{file_path}': {e}"))
        })?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(header)
            .delimiter(delimiter)
            .trim(csv::Trim::All)
            .flexible(true)
            .from_reader(BufReader::new(file));
        let headers: Vec<String> = if header {
            reader
                .headers()
                .map_err(|e| QueryError::execution(format!("COPY CSV header error: {e}")))?
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self {
            headers,
            reader: reader.into_records(),
            finished: false,
        })
    }

    /// Header names when the import declared `HEADER`; empty otherwise.
    fn headers(&self) -> &[String] {
        &self.headers
    }

    fn is_eof(&self) -> bool {
        self.finished
    }

    /// Pull up to `batch_size` records as plain string rows.
    ///
    /// Records whose field count differs from the expected width are counted
    /// as malformed and skipped (reported once at the end of the import).
    fn next_batch(
        &mut self,
        batch_size: usize,
        mapping: &ColumnMapping,
        skipped: &mut u64,
    ) -> Result<Vec<Vec<String>>, QueryError> {
        let mut out: Vec<Vec<String>> = Vec::with_capacity(batch_size);
        while out.len() < batch_size {
            let Some(record) = self.next_record()? else {
                break;
            };
            if record.len() != mapping.expected_width {
                *skipped += 1;
                continue;
            }
            out.push(record.iter().map(|s| s.to_string()).collect());
        }
        Ok(out)
    }

    fn next_record(&mut self) -> Result<Option<csv::StringRecord>, QueryError> {
        if self.finished {
            return Ok(None);
        }
        match self.reader.next() {
            Some(Ok(rec)) => Ok(Some(rec)),
            Some(Err(e)) => Err(QueryError::execution(format!("COPY CSV read error: {e}"))),
            None => {
                self.finished = true;
                Ok(None)
            }
        }
    }
}

/// Parse a raw CSV cell into a typed value.
///
/// Inference order: empty → NULL, boolean literals, integers, floats, then
/// string. The storage layer normalizes values against the tag / edge-type
/// schema on insert.
fn parse_copy_value(s: &str) -> Value {
    if s.is_empty() {
        return Value::Null(crate::core::value::NullType::Null);
    }
    if s.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if s.eq_ignore_ascii_case("null") {
        return Value::Null(crate::core::value::NullType::Null);
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::BigInt(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return Value::Double(f);
    }
    Value::string(s)
}

// ── COPY TO: CSV export ──────────────────────────────────────────────────────

/// Execute a COPY TO statement and return the number of rows exported.
///
/// Vertices are written as `vid` followed by the tag's schema property
/// columns; edges as `src`, `dst` followed by the edge-type property columns.
/// NULL values become empty cells.
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
                let props = vertex
                    .get_tag(tag)
                    .map(|t| &t.properties)
                    .unwrap_or(&vertex.properties);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn vertex_header_mapping_locates_vid_and_properties() {
        let h = headers(&["name", "vid", "age"]);
        let m = build_column_mapping(&h, true, None, KeyLayout::Vertex).expect("mapping");
        assert_eq!(m.vid_index, 1);
        assert_eq!(
            m.property_indices,
            vec![(0, "name".to_string()), (2, "age".to_string())]
        );
        assert_eq!(m.expected_width, 3);
    }

    #[test]
    fn vertex_header_without_id_column_errors() {
        let h = headers(&["name", "age"]);
        assert!(build_column_mapping(&h, true, None, KeyLayout::Vertex).is_err());
    }

    #[test]
    fn edge_header_named_endpoints() {
        let h = headers(&["since", "dst", "src"]);
        let m = build_column_mapping(&h, true, None, KeyLayout::Edge).expect("mapping");
        assert_eq!(m.src_index, 2);
        assert_eq!(m.dst_index, 1);
        assert_eq!(m.property_indices, vec![(0, "since".to_string())]);
    }

    #[test]
    fn edge_header_only_src_named_errors() {
        let h = headers(&["src", "since"]);
        assert!(build_column_mapping(&h, true, None, KeyLayout::Edge).is_err());
    }

    #[test]
    fn edge_header_no_named_endpoints_falls_back_positional() {
        let h = headers(&["a", "b", "since"]);
        let m = build_column_mapping(&h, true, None, KeyLayout::Edge).expect("mapping");
        assert_eq!(m.src_index, 0);
        assert_eq!(m.dst_index, 1);
        assert_eq!(m.property_indices, vec![(2, "since".to_string())]);
    }

    #[test]
    fn positional_layout_uses_schema_order() {
        let schema = vec!["name".to_string(), "age".to_string()];
        let m =
            build_column_mapping(&[], false, Some(&schema), KeyLayout::Vertex).expect("mapping");
        assert_eq!(m.vid_index, 0);
        assert_eq!(
            m.property_indices,
            vec![(1, "name".to_string()), (2, "age".to_string())]
        );
        assert_eq!(m.expected_width, 3);
    }

    #[test]
    fn parse_value_inference() {
        assert_eq!(
            parse_copy_value(""),
            Value::Null(crate::core::value::NullType::Null)
        );
        assert_eq!(parse_copy_value("true"), Value::Bool(true));
        assert_eq!(parse_copy_value("42"), Value::BigInt(42));
        assert_eq!(parse_copy_value("4.5"), Value::Double(4.5));
        assert_eq!(parse_copy_value("hello"), Value::string("hello"));
    }
}
