use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::wal::EntityRef;
use crate::core::{Value, Vertex, Edge};
use crate::query::executor::base::{MemoryBudget, MemoryReservation};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::storage::FlatVertexRecord;
use std::collections::HashMap;

/// Convert an EntityRef to VertexId for back-to-table fetches.
pub(crate) fn entity_ref_to_vertex_id(entity_ref: &EntityRef) -> Option<VertexId> {
    match entity_ref {
        EntityRef::Vertex(vid) => Some(*vid),
        EntityRef::Edge { .. } => None,
    }
}

/// Build a flat scan row from a storage flat vertex record.
///
/// Slot 0 keeps the `Value::Vertex` rebuilt from the record (consumed by graph
/// operators, `RETURN p`, and label checks); the flat property columns are
/// extracted through the shared [`Vertex::property_value`] semantics so the
/// flat path cannot diverge from the per-row evaluator.
pub(crate) fn make_flat_vertex_record_row(
    record: FlatVertexRecord,
    flatten: &[String],
) -> Vec<Value> {
    let properties: HashMap<String, Value> = record.props.into_iter().collect();
    let tags = if record.tag_name.is_empty() {
        Vec::new()
    } else {
        vec![crate::core::Tag::new(record.tag_name, properties.clone())]
    };
    let vertex = Vertex {
        vid: record.vid,
        id: record.internal_id,
        tags,
        properties,
    };
    make_flat_vertex_row(vertex, flatten)
}

pub(crate) fn make_flat_vertex_row(vertex: Vertex, flatten: &[String]) -> Vec<Value> {
    let props: Vec<Value> = flatten
        .iter()
        .map(|prop| {
            vertex
                .property_value(prop)
                .unwrap_or_else(|| Value::Null(crate::core::value::NullType::Null))
        })
        .collect();
    let mut row = Vec::with_capacity(flatten.len() + 1);
    row.push(Value::Vertex(Box::new(vertex)));
    row.extend(props);
    row
}

/// Flat covering-row variant: synthesizes a vertex from the covering columns
/// and appends the columns as property slots after it.
pub(crate) fn make_flat_covering_vertex_row(
    entity_ref: &EntityRef,
    columns: Vec<(String, Value)>,
    flatten: &[String],
) -> Option<Vec<Value>> {
    let vertex_id = entity_ref_to_vertex_id(entity_ref)?;
    let vertex = Vertex::new_with_properties(vertex_id, Vec::new(), columns.into_iter().collect());
    Some(make_flat_vertex_row(vertex, flatten))
}

pub(crate) fn make_edge_row(edge: Edge) -> Vec<Value> {
    vec![Value::Edge(Box::new(edge))]
}

pub(crate) fn make_flat_edge_row(edge: Edge, flatten: &[String]) -> Vec<Value> {    let props: Vec<Value> = flatten
        .iter()
        .map(|prop| {
            edge.properties()
                .get(prop)
                .cloned()
                .unwrap_or_else(|| Value::Null(crate::core::value::NullType::Null))
        })
        .collect();
    let mut row = Vec::with_capacity(flatten.len() + 1);
    row.push(Value::Edge(Box::new(edge)));
    row.extend(props);
    row
}

/// Flat covering-row variant: synthesizes an edge from the covering columns
/// and appends the columns as property slots after it.
pub(crate) fn make_flat_covering_edge_row(
    entity_ref: &EntityRef,
    columns: Vec<(String, Value)>,
    edge_type: String,
    flatten: &[String],
) -> Option<Vec<Value>> {
    let EntityRef::Edge {
        src, dst, ranking, ..
    } = entity_ref
    else {
        return None;
    };
    let mut edge = Edge::new_empty(*src, *dst, edge_type, *ranking);
    for (name, value) in columns {
        edge.set_property(name, value);
    }
    Some(make_flat_edge_row(edge, flatten))
}

pub(crate) fn storage_error(
    source: &str,
    operation: &str,
    space_name: &str,
    error: impl std::fmt::Display,
) -> QueryError {
    QueryError::execution(format!(
        "{} {} failed for space '{}': {}",
        source, operation, space_name, error
    ))
}

pub(crate) fn parse_vertex_id(value: &str) -> VertexId {
    value
        .parse::<i64>()
        .map(VertexId::from_int64)
        .unwrap_or_else(|_| VertexId::from_string(value.to_string()))
}

pub(crate) fn reserve_memory(
    base: &OperatorBase,
    rows: &[Vec<Value>],
) -> Result<Option<MemoryReservation>, QueryError> {
    reserve_memory_with_extra(base, rows, 0)
}

/// Reserve memory for `rows` plus `extra_bytes` (e.g. the typed column
/// layout built by the source) against the query memory budget.
pub(crate) fn reserve_memory_with_extra(
    base: &OperatorBase,
    rows: &[Vec<Value>],
    extra_bytes: usize,
) -> Result<Option<MemoryReservation>, QueryError> {
    let Some(runtime) = base.runtime.as_ref() else {
        return Ok(None);
    };
    let bytes = MemoryBudget::estimate_rows_memory(rows).saturating_add(extra_bytes);
    runtime.memory_budget.reserve(bytes).map(Some)
}

/// Attach the query-level columnar fast-path counters to a produced chunk
/// (T5 observability) when the operator has a runtime attached.
pub(crate) fn attach_columnar_stats(base: &OperatorBase, chunk: DataChunk) -> DataChunk {
    match base.runtime.as_ref() {
        Some(rt) => chunk.with_columnar_stats(rt.columnar_stats()),
        None => chunk,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::Tag;

    #[test]
    fn flat_vertex_row_contains_projected_properties_in_order() {
        let mut vertex = Vertex::with_vid(VertexId::from_int64(1));
        vertex.set_vertex_property("age".to_string(), Value::BigInt(30));
        vertex.set_vertex_property("name".to_string(), Value::string("Alice"));
        let row = make_flat_vertex_row(
            vertex,
            &["age".to_string(), "name".to_string()],
        );
        assert_eq!(row.len(), 3);
        assert!(matches!(&row[0], Value::Vertex(_)));
        assert_eq!(row[1], Value::BigInt(30));
        assert_eq!(row[2], Value::string("Alice"));
    }

    #[test]
    fn flat_vertex_row_empty_flatten_keeps_single_entity_column() {
        let vertex = Vertex::with_vid(VertexId::from_int64(1));
        let row = make_flat_vertex_row(vertex, &[]);
        assert_eq!(row.len(), 1);
        assert!(matches!(&row[0], Value::Vertex(_)));
    }

    #[test]
    fn flat_vertex_row_missing_property_is_null() {
        let vertex = Vertex::with_vid(VertexId::from_int64(1));
        let row = make_flat_vertex_row(vertex, &["missing".to_string()]);
        assert_eq!(row.len(), 2);
        assert!(matches!(&row[1], Value::Null(_)));
    }

    #[test]
    fn flat_vertex_row_reads_tag_properties_and_tag_name() {
        let mut vertex = Vertex::with_vid(VertexId::from_int64(1));
        vertex.add_tag(Tag::new(
            "person".to_string(),
            [("city".to_string(), Value::string("NYC"))]
                .into_iter()
                .collect(),
        ));
        // Tag property fallback mirrors eval_property_access semantics.
        let row = make_flat_vertex_row(vertex.clone(), &["city".to_string()]);
        assert_eq!(row[1], Value::string("NYC"));
        // A tag whose name equals the property yields the tag's property map.
        let row = make_flat_vertex_row(vertex, &["person".to_string()]);
        assert!(matches!(&row[1], Value::Map(_)));
    }

    #[test]
    fn flat_edge_row_contains_projected_properties() {
        let mut edge = Edge::new_empty(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "friend".to_string(),
            0,
        );
        edge.set_property("since".to_string(), Value::BigInt(2024));
        let row = make_flat_edge_row(edge, &["since".to_string(), "missing".to_string()]);
        assert_eq!(row.len(), 3);
        assert!(matches!(&row[0], Value::Edge(_)));
        assert_eq!(row[1], Value::BigInt(2024));
        assert!(matches!(&row[2], Value::Null(_)));
    }

    #[test]
    fn flat_vertex_record_row_rebuilds_vertex_and_flat_columns() {
        let record = FlatVertexRecord {
            vid: VertexId::from_int64(42),
            internal_id: 7,
            tag_name: "person".to_string(),
            props: vec![
                ("age".to_string(), Value::BigInt(30)),
                ("name".to_string(), Value::string("Alice")),
            ],
        };
        let row = make_flat_vertex_record_row(record, &["name".to_string(), "age".to_string()]);
        assert_eq!(row.len(), 3);
        let Value::Vertex(vertex) = &row[0] else {
            panic!("slot 0 must hold the rebuilt vertex");
        };
        assert_eq!(vertex.vid, VertexId::from_int64(42));
        assert_eq!(vertex.id, 7);
        assert_eq!(vertex.tags.len(), 1);
        assert_eq!(vertex.tags[0].name, "person");
        assert_eq!(vertex.get_property_any("age"), Some(&Value::BigInt(30)));
        assert_eq!(row[1], Value::string("Alice"));
        assert_eq!(row[2], Value::BigInt(30));
    }

    #[test]
    fn flat_vertex_record_row_missing_property_is_null() {
        let record = FlatVertexRecord {
            vid: VertexId::from_int64(42),
            internal_id: 7,
            tag_name: "person".to_string(),
            props: vec![("age".to_string(), Value::BigInt(30))],
        };
        let row = make_flat_vertex_record_row(record, &["missing".to_string()]);
        assert_eq!(row.len(), 2);
        assert!(matches!(&row[1], Value::Null(_)));
    }

    #[test]
    fn flat_vertex_record_row_tag_name_equals_property_yields_tag_map() {
        // Mirrors Vertex::property_value semantics: a tag whose name equals
        // the property yields the tag's property map.
        let record = FlatVertexRecord {
            vid: VertexId::from_int64(42),
            internal_id: 7,
            tag_name: "person".to_string(),
            props: vec![("age".to_string(), Value::BigInt(30))],
        };
        let row = make_flat_vertex_record_row(record, &["person".to_string()]);
        assert!(matches!(&row[1], Value::Map(_)));
    }
}