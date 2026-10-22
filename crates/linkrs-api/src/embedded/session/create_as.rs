//! `CREATE TAG/EDGE <name> AS (<query>)` materialization.
//!
//! The inner query is run on this session, its output columns drive the new
//! table schema, and one vertex / edge is bulk-loaded per result row. The
//! supporting type-inference helpers live here too.

use super::Session;
use crate::api_core::types::PropertyDef;
use crate::api_core::{CoreError, CoreResult};
use crate::embedded::result::QueryResult;
use crate::embedded::result::Row;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::DataType;
use linkrs_core::Edge;
use linkrs_core::Value;
use linkrs_core::Vertex;
use linkrs_query::parser::ast::stmt::CreateStmt;
use linkrs_query::parser::ast::stmt::CreateTarget;
use linkrs_query::parser::ast::Stmt;
use linkrs_query::parser::Parser;
use linkrs_storage::StorageClient;
use std::collections::HashMap;
use std::sync::Arc;

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Detect `CREATE TAG/EDGE ... AS (query)` statements.
    ///
    /// Gated on the leading `CREATE` keyword so ordinary statements skip the
    /// API-layer parse; the parsed AST target then decides whether the
    /// statement materializes a table from a query. Malformed input yields
    /// `None` and falls through to the normal pipeline for error reporting.
    pub(crate) fn parse_create_as(query: &str) -> Option<CreateStmt> {
        if !Self::starts_with_create(query) {
            return None;
        }
        let mut parser = Parser::new(query);
        match parser.parse() {
            Ok(result) if !parser.has_errors() => match result.ast.stmt() {
                Stmt::Create(stmt) => match &stmt.target {
                    CreateTarget::TagAsQuery { .. } | CreateTarget::EdgeAsQuery { .. } => {
                        Some(stmt.clone())
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether the statement opens with the `CREATE` keyword.
    pub(crate) fn starts_with_create(query: &str) -> bool {
        query
            .trim_start()
            .split_whitespace()
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("CREATE"))
    }

    /// Execute `CREATE TAG/EDGE <name> AS (<query>)`.
    ///
    /// Runs the inner query on this session, derives the new table schema
    /// from its output columns, creates the table, and bulk-loads one
    /// vertex or edge per result row. Vertex imports require a `vid`/`id`
    /// column; edge imports require `src` and `dst` columns.
    pub(crate) fn execute_create_as(&self, stmt: &CreateStmt) -> CoreResult<QueryResult> {
        use linkrs_query::parser::ast::stmt::CreateTarget;

        let (name, query_text, is_edge) = match &stmt.target {
            CreateTarget::TagAsQuery { name, query_text } => {
                (name.clone(), query_text.clone(), false)
            }
            CreateTarget::EdgeAsQuery { name, query_text } => {
                (name.clone(), query_text.clone(), true)
            }
            _ => {
                return Err(CoreError::InvalidParameter(
                    "Not a CREATE ... AS statement".to_string(),
                ));
            }
        };
        if name.trim().is_empty() {
            return Err(CoreError::InvalidParameter(
                "CREATE ... AS requires a table name".to_string(),
            ));
        }
        if query_text.trim().is_empty() {
            return Err(CoreError::InvalidParameter(
                "CREATE ... AS requires a subquery".to_string(),
            ));
        }
        if self.current_transaction.read().is_some() {
            return Err(CoreError::InvalidParameter(
                "CREATE ... AS cannot run inside an explicit transaction".to_string(),
            ));
        }
        if Self::parse_create_as(&query_text).is_some() {
            return Err(CoreError::InvalidParameter(
                "Nested CREATE ... AS is not supported".to_string(),
            ));
        }

        let inner = self.execute(&query_text)?;
        let columns = inner.columns().to_vec();
        if columns.is_empty() {
            return Err(CoreError::InvalidParameter(
                "CREATE ... AS inner query returned no columns".to_string(),
            ));
        }

        let space_name = self
            .space_name()
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;
        let space_id = match *self.space_id.read() {
            Some(id) => id,
            None => self.db.schema_api.use_space(&space_name)?,
        };

        if is_edge {
            self.create_edge_as(
                &space_name,
                space_id,
                &name,
                stmt.if_not_exists,
                &columns,
                &inner,
            )
        } else {
            self.create_tag_as(
                &space_name,
                space_id,
                &name,
                stmt.if_not_exists,
                &columns,
                &inner,
            )
        }
    }

    /// Materialize inner query rows as a new tag (vertex table).
    fn create_tag_as(
        &self,
        space_name: &str,
        space_id: u64,
        name: &str,
        if_not_exists: bool,
        columns: &[String],
        inner: &QueryResult,
    ) -> CoreResult<QueryResult> {
        let vid_col = find_result_column(columns, &["vid", "id", "_id", "vertex_id"])
            .ok_or_else(|| {
                CoreError::InvalidParameter(
                    "CREATE TAG ... AS requires the inner query to output a vertex id column (vid/id)"
                        .to_string(),
                )
            })?;
        let prop_cols: Vec<String> = columns.iter().filter(|c| *c != &vid_col).cloned().collect();

        let exists = self
            .db
            .storage
            .read()
            .get_tag(space_name, name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?
            .is_some();
        if exists {
            if if_not_exists {
                return Ok(Self::single_message_result(format!(
                    "Tag '{name}' already exists"
                )));
            }
            return Err(CoreError::InvalidParameter(format!(
                "Tag '{name}' already exists"
            )));
        }

        let properties = infer_result_properties(&prop_cols, inner.rows());
        self.db
            .schema_api
            .create_tag(space_id, name, properties)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        let mut vertices = Vec::with_capacity(inner.len());
        for row in inner.rows() {
            let vid_value = row.get(&vid_col).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE TAG ... AS: missing vertex id in column '{vid_col}'"
                ))
            })?;
            let vid = vid_from_value(vid_value).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE TAG ... AS: value in column '{vid_col}' cannot be used as a vertex id"
                ))
            })?;
            let mut props: HashMap<Arc<str>, Value> = std::collections::HashMap::new();
            for col in &prop_cols {
                if let Some(value) = row.get(col) {
                    ensure_scalar_property(col, value)?;
                    if matches!(value, Value::Null(_) | Value::Empty) {
                        continue;
                    }
                    props.insert(col.as_str().into(), value.clone());
                }
            }
            vertices.push(Vertex::new(
                vid,
                linkrs_core::Tag::new(name.to_string(), props),
            ));
        }
        let count = self.batch_insert_vertices(vertices)?;
        self.statistics.record_changes(count as u64);
        Ok(Self::single_message_result(format!(
            "Created tag '{name}' with {count} vertices from query"
        )))
    }

    /// Materialize inner query rows as a new edge type.
    fn create_edge_as(
        &self,
        space_name: &str,
        space_id: u64,
        name: &str,
        if_not_exists: bool,
        columns: &[String],
        inner: &QueryResult,
    ) -> CoreResult<QueryResult> {
        let src_col = find_result_column(columns, &["src", "_src", "source"]);
        let dst_col = find_result_column(columns, &["dst", "_dst", "destination", "dest"]);
        let (src_col, dst_col) = match (src_col, dst_col) {
            (Some(s), Some(d)) => {
                if s == d {
                    return Err(CoreError::InvalidParameter(
                        "CREATE EDGE ... AS: src and dst resolve to the same column".to_string(),
                    ));
                }
                (s, d)
            }
            (None, None) => {
                return Err(CoreError::InvalidParameter(
                    "CREATE EDGE ... AS requires the inner query to output src and dst columns"
                        .to_string(),
                ));
            }
            (Some(_), None) => {
                return Err(CoreError::InvalidParameter(
                    "CREATE EDGE ... AS: inner query names a source column but no destination column (dst)"
                        .to_string(),
                ));
            }
            (None, Some(_)) => {
                return Err(CoreError::InvalidParameter(
                    "CREATE EDGE ... AS: inner query names a destination column but no source column (src)"
                        .to_string(),
                ));
            }
        };
        let prop_cols: Vec<String> = columns
            .iter()
            .filter(|c| *c != &src_col && *c != &dst_col)
            .cloned()
            .collect();

        let exists = self
            .db
            .storage
            .read()
            .get_edge_type(space_name, name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?
            .is_some();
        if exists {
            if if_not_exists {
                return Ok(Self::single_message_result(format!(
                    "Edge type '{name}' already exists"
                )));
            }
            return Err(CoreError::InvalidParameter(format!(
                "Edge type '{name}' already exists"
            )));
        }

        let properties = infer_result_properties(&prop_cols, inner.rows());
        self.db
            .schema_api
            .create_edge_type(space_id, name, properties)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        let mut edges = Vec::with_capacity(inner.len());
        for row in inner.rows() {
            let src_value = row.get(&src_col).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE EDGE ... AS: missing source id in column '{src_col}'"
                ))
            })?;
            let dst_value = row.get(&dst_col).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE EDGE ... AS: missing destination id in column '{dst_col}'"
                ))
            })?;
            let src = vid_from_value(src_value).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE EDGE ... AS: value in column '{src_col}' cannot be used as a vertex id"
                ))
            })?;
            let dst = vid_from_value(dst_value).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE EDGE ... AS: value in column '{dst_col}' cannot be used as a vertex id"
                ))
            })?;
            let mut props: HashMap<Arc<str>, Value> = std::collections::HashMap::new();
            for col in &prop_cols {
                if let Some(value) = row.get(col) {
                    ensure_scalar_property(col, value)?;
                    if matches!(value, Value::Null(_) | Value::Empty) {
                        continue;
                    }
                    props.insert(col.as_str().into(), value.clone());
                }
            }
            edges.push(Edge::new(src, dst, name.to_string(), 0, props));
        }
        let count = self.batch_insert_edges(edges)?;
        self.statistics.record_changes(count as u64);
        Ok(Self::single_message_result(format!(
            "Created edge type '{name}' with {count} edges from query"
        )))
    }
}

/// Locate a result column by case-insensitive name among several aliases.
fn find_result_column(columns: &[String], aliases: &[&str]) -> Option<String> {
    columns
        .iter()
        .find(|c| aliases.iter().any(|a| c.eq_ignore_ascii_case(a)))
        .cloned()
}

/// Derive property definitions from query result columns.
///
/// Each column takes the type of its first non-null value; columns that
/// only carry nulls default to strings.
fn infer_result_properties(columns: &[String], rows: &[Row]) -> Vec<PropertyDef> {
    columns
        .iter()
        .map(|col| {
            let mut data_type = DataType::String;
            for row in rows {
                match row.get(col) {
                    Some(Value::Null(_)) | Some(Value::Empty) | None => continue,
                    Some(value) => {
                        data_type = infer_result_data_type(value);
                        break;
                    }
                }
            }
            PropertyDef {
                name: col.clone(),
                data_type,
                nullable: true,
                default_value: None,
                comment: None,
            }
        })
        .collect()
}

/// Map a result value to the table column type used for `CREATE ... AS`.
fn infer_result_data_type(value: &Value) -> DataType {
    use linkrs_core::DataType;
    match value {
        Value::Null(_) | Value::Empty => DataType::String,
        Value::Bool(_) => DataType::Bool,
        Value::SmallInt(_) => DataType::SmallInt,
        Value::Int(_) => DataType::Int,
        Value::BigInt(_) => DataType::BigInt,
        Value::Float(_) => DataType::Float,
        Value::Double(_) => DataType::Double,
        Value::Decimal128(_) => DataType::Decimal128,
        Value::String(_) | Value::FixedString(_) => DataType::String,
        Value::Date(_) => DataType::Date,
        Value::Time(_) => DataType::Time,
        Value::DateTime(_) => DataType::DateTime,
        Value::Uuid(_) => DataType::Uuid,
        Value::Json(_) => DataType::Json,
        Value::JsonB(_) => DataType::JsonB,
        Value::Blob(_) => DataType::Blob,
        Value::List(_) => DataType::List(Box::new(DataType::Empty)),
        Value::Map(_) => DataType::Map(Box::new(DataType::Empty)),
        Value::Set(_) => DataType::Set(Box::new(DataType::Empty)),
        _ => DataType::String,
    }
}

/// Reject graph and container values as table property columns.
fn ensure_scalar_property(col: &str, value: &Value) -> CoreResult<()> {
    let complex = matches!(
        value,
        Value::Vertex(_)
            | Value::Edge(_)
            | Value::Path(_)
            | Value::List(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::DataSet(_)
            | Value::Struct(_)
            | Value::Array(_)
            | Value::Vector(_)
            | Value::VertexId(_)
            | Value::EdgeId(_)
    );
    if complex {
        return Err(CoreError::InvalidParameter(format!(
            "CREATE ... AS: column '{col}' carries a graph or container value; project scalar properties instead"
        )));
    }
    Ok(())
}

/// Read a vertex id from a result value.
///
/// Numeric values map to the integer id domain, strings become string ids,
/// and vertex values contribute their own id.
fn vid_from_value(value: &Value) -> Option<VertexId> {
    use linkrs_core::types::storage_ids::VertexId;
    match value {
        Value::SmallInt(i) => VertexId::try_from_int64(i64::from(*i)).ok(),
        Value::Int(i) => VertexId::try_from_int64(i64::from(*i)).ok(),
        Value::BigInt(i) => VertexId::try_from_int64(*i).ok(),
        Value::String(s) => VertexId::try_from_string(s.as_str()).ok(),
        Value::FixedString(s) => VertexId::try_from_string(s.as_str()).ok(),
        Value::Vertex(v) => Some(*v.vid()),
        Value::VertexId(id) => Some(*id),
        _ => None,
    }
}
