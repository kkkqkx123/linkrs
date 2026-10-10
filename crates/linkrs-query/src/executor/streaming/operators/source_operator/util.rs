use crate::executor::base::{MemoryBudget, MemoryReservation};
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::storage::FlatVertexRecord;
use linkrs_core::error::QueryError;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::wal::EntityRef;
use linkrs_core::{Edge, Value, Vertex};
use std::collections::HashMap;
use std::sync::Arc;

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
    flatten: &[Arc<str>],
) -> Vec<Value> {
    let properties: HashMap<Arc<str>, Value> = record.props.into_iter().collect();
    let vertex = Vertex::new(
        record.vid,
        linkrs_core::Tag::new(record.tag_name, properties),
    );
    make_flat_vertex_row(vertex, flatten)
}

/// Identity variant of [`make_flat_vertex_record_row`]: the entity column
/// carries the vertex id without building the per-row property map or
/// boxing a `Vertex`. Flat property columns are read straight from the
/// record's property list. Only valid when every downstream property read
/// is served by the flat slots (enforced by the scan identity annotation).
pub(crate) fn make_flat_vertex_record_identity_row(
    record: FlatVertexRecord,
    flatten: &[Arc<str>],
) -> Vec<Value> {
    let null = Value::Null(linkrs_core::value::NullType::Null);
    let mut row = Vec::with_capacity(flatten.len() + 1);
    row.push(Value::VertexId(record.vid));
    for prop in flatten {
        let value = record
            .props
            .iter()
            .find(|(name, _)| name.as_ref() == prop.as_ref())
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| null.clone());
        row.push(value);
    }
    row
}

pub(crate) fn make_flat_vertex_row(vertex: Vertex, flatten: &[Arc<str>]) -> Vec<Value> {
    let props: Vec<Value> = flatten
        .iter()
        .map(|prop| {
            vertex
                .property_value(prop)
                .unwrap_or_else(|| Value::Null(linkrs_core::value::NullType::Null))
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
    tag: &str,
    columns: Vec<(Arc<str>, Value)>,
    flatten: &[Arc<str>],
) -> Option<Vec<Value>> {
    let vertex_id = entity_ref_to_vertex_id(entity_ref)?;
    let vertex = Vertex::new(
        vertex_id,
        linkrs_core::Tag::new(tag.to_string(), columns.into_iter().collect()),
    );
    Some(make_flat_vertex_row(vertex, flatten))
}

pub(crate) fn make_edge_row(edge: Edge) -> Vec<Value> {
    vec![Value::Edge(Box::new(edge))]
}

/// Identity variant of [`make_flat_edge_row`]: the entity column carries
/// the edge header without building the per-row property map or boxing an
/// `Edge`. Flat property columns are read straight from the edge's decoded
/// properties. Only valid when every downstream property read is served by
/// the flat slots (enforced by the scan edge identity annotation).
pub(crate) fn make_flat_edge_identity_row(edge: Edge, flatten: &[Arc<str>]) -> Vec<Value> {
    let null = Value::Null(linkrs_core::value::NullType::Null);
    let mut row = Vec::with_capacity(flatten.len() + 1);
    row.push(Value::edge_header(linkrs_core::EdgeHeader::from_edge(
        &edge,
    )));
    for prop in flatten {
        let value = edge
            .properties()
            .get(prop.as_ref())
            .cloned()
            .unwrap_or_else(|| null.clone());
        row.push(value);
    }
    row
}

pub(crate) fn make_flat_edge_row(edge: Edge, flatten: &[Arc<str>]) -> Vec<Value> {
    let props: Vec<Value> = flatten
        .iter()
        .map(|prop| {
            edge.properties()
                .get(prop.as_ref())
                .cloned()
                .unwrap_or_else(|| Value::Null(linkrs_core::value::NullType::Null))
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
    columns: Vec<(Arc<str>, Value)>,
    edge_type: String,
    flatten: &[Arc<str>],
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

pub(crate) fn parse_vertex_id(value: &str) -> Result<VertexId, QueryError> {
    if let Ok(parsed) = value.parse::<i64>() {
        // Numeric input is an integer id: a negative value is rejected,
        // never reinterpreted as a text id.
        return VertexId::try_from_int64(parsed)
            .map_err(|e| QueryError::execution(format!("Invalid vertex id '{value}': {e}")));
    }
    VertexId::try_from_string(value)
        .map_err(|e| QueryError::execution(format!("Invalid vertex id '{value}': {e}")))
}

pub(crate) fn reserve_memory(
    runtime: &Option<Arc<ExecutionRuntime>>,
    rows: &[Vec<Value>],
) -> Result<Option<MemoryReservation>, QueryError> {
    reserve_memory_with_extra(runtime, rows, 0)
}

/// Reserve memory for `rows` plus `extra_bytes` (e.g. the typed column
/// layout built by the source) against the query memory budget.
pub(crate) fn reserve_memory_with_extra(
    runtime: &Option<Arc<ExecutionRuntime>>,
    rows: &[Vec<Value>],
    extra_bytes: usize,
) -> Result<Option<MemoryReservation>, QueryError> {
    let Some(runtime) = runtime.as_ref() else {
        return Ok(None);
    };
    let bytes = MemoryBudget::estimate_rows_memory(rows).saturating_add(extra_bytes);
    runtime.memory_budget.reserve(bytes).map(Some)
}

/// Attach the query-level columnar fast-path counters to a produced chunk
/// (T5 observability) when the operator has a runtime attached.
pub(crate) fn attach_columnar_stats(
    runtime: &Option<Arc<ExecutionRuntime>>,
    chunk: DataChunk,
) -> DataChunk {
    match runtime.as_ref() {
        Some(rt) => chunk.with_columnar_stats(rt.columnar_stats()),
        None => chunk,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linkrs_core::types::storage_ids::VertexId;
    use linkrs_core::Tag;
    use std::collections::HashMap;

    fn test_vertex_with_props(props: Vec<(String, Value)>) -> Vertex {
        let properties: HashMap<Arc<str>, Value> = props
            .into_iter()
            .map(|(name, value)| (Arc::from(name.as_str()), value))
            .collect();
        Vertex::new(
            VertexId::try_from_int64(1).expect("valid vertex id"),
            Tag::new(String::new(), properties),
        )
    }

    #[test]
    fn flat_vertex_row_contains_projected_properties_in_order() {
        let vertex = Vertex::new(
            VertexId::try_from_int64(1).expect("valid vertex id"),
            Tag::new(
                String::new(),
                HashMap::from([
                    ("age".into(), Value::BigInt(30)),
                    ("name".into(), Value::string("Alice")),
                ]),
            ),
        );
        let row = make_flat_vertex_row(vertex, &[Arc::from("age"), Arc::from("name")]);
        assert_eq!(row.len(), 3);
        assert!(matches!(&row[0], Value::Vertex(_)));
        assert_eq!(row[1], Value::BigInt(30));
        assert_eq!(row[2], Value::string("Alice"));
    }

    #[test]
    fn flat_vertex_row_empty_flatten_keeps_single_entity_column() {
        let vertex = test_vertex_with_props(vec![]);
        let row = make_flat_vertex_row(vertex, &[]);
        assert_eq!(row.len(), 1);
        assert!(matches!(&row[0], Value::Vertex(_)));
    }

    #[test]
    fn flat_vertex_row_missing_property_is_null() {
        let vertex = test_vertex_with_props(vec![]);
        let row = make_flat_vertex_row(vertex, &[Arc::from("missing")]);
        assert_eq!(row.len(), 2);
        assert!(matches!(&row[1], Value::Null(_)));
    }

    #[test]
    fn flat_vertex_row_reads_tag_properties_and_tag_name() {
        let vertex = Vertex::new(
            VertexId::try_from_int64(1).expect("valid vertex id"),
            Tag::new(
                "person".to_string(),
                [("city".into(), Value::string("NYC"))]
                    .into_iter()
                    .collect(),
            ),
        );
        // Tag property fallback mirrors eval_property_access semantics.
        let row = make_flat_vertex_row(vertex.clone(), &[Arc::from("city")]);
        assert_eq!(row[1], Value::string("NYC"));
        // The tag-name-yields-map implicit behavior is cancelled: a property
        // name that only equals the tag resolves to Null.
        let row = make_flat_vertex_row(vertex, &[Arc::from("person")]);
        assert!(matches!(&row[1], Value::Null(_)));
    }

    #[test]
    fn flat_edge_row_contains_projected_properties() {
        let mut edge = Edge::new_empty(
            VertexId::try_from_int64(1).expect("valid vertex id"),
            VertexId::try_from_int64(2).expect("valid vertex id"),
            "friend".to_string(),
            0,
        );
        edge.set_property("since".into(), Value::BigInt(2024));
        let row = make_flat_edge_row(edge, &[Arc::from("since"), Arc::from("missing")]);
        assert_eq!(row.len(), 3);
        assert!(matches!(&row[0], Value::Edge(_)));
        assert_eq!(row[1], Value::BigInt(2024));
        assert!(matches!(&row[2], Value::Null(_)));
    }

    #[test]
    fn flat_vertex_record_row_rebuilds_vertex_and_flat_columns() {
        let record = FlatVertexRecord {
            vid: VertexId::try_from_int64(42).expect("valid vertex id"),
            internal_id: 7,
            tag_name: "person".to_string(),
            props: vec![
                ("age".into(), Value::BigInt(30)),
                ("name".into(), Value::string("Alice")),
            ],
        };
        let row = make_flat_vertex_record_row(record, &[Arc::from("name"), Arc::from("age")]);
        assert_eq!(row.len(), 3);
        let Value::Vertex(vertex) = &row[0] else {
            panic!("slot 0 must hold the rebuilt vertex");
        };
        assert_eq!(
            vertex.vid,
            VertexId::try_from_int64(42).expect("valid vertex id")
        );
        assert_eq!(vertex.tag.name, "person");
        assert_eq!(vertex.tag_name(), "person");
        assert_eq!(vertex.property_value("age"), Some(Value::BigInt(30)));
        assert_eq!(row[1], Value::string("Alice"));
        assert_eq!(row[2], Value::BigInt(30));
    }

    #[test]
    fn flat_vertex_record_identity_row_emits_id_without_boxing() {
        let record = FlatVertexRecord {
            vid: VertexId::try_from_int64(42).expect("valid vertex id"),
            internal_id: 7,
            tag_name: "person".to_string(),
            props: vec![
                ("age".into(), Value::BigInt(30)),
                ("name".into(), Value::string("Alice")),
            ],
        };
        let row = make_flat_vertex_record_identity_row(record, &[Arc::from("name")]);
        assert_eq!(row.len(), 2);
        assert_eq!(
            row[0],
            Value::VertexId(VertexId::try_from_int64(42).expect("valid vertex id"))
        );
        assert_eq!(row[1], Value::string("Alice"));
    }

    #[test]
    fn flat_edge_identity_row_emits_header_without_boxing() {
        let mut edge = Edge::new_empty(
            VertexId::try_from_int64(1).expect("valid vertex id"),
            VertexId::try_from_int64(2).expect("valid vertex id"),
            "friend".to_string(),
            3,
        );
        edge.set_property("since".into(), Value::BigInt(2024));
        let row = make_flat_edge_identity_row(edge, &[Arc::from("since")]);
        assert_eq!(row.len(), 2);
        let Value::EdgeHeader(header) = &row[0] else {
            panic!("slot 0 must hold the edge header");
        };
        assert_eq!(
            header.src,
            VertexId::try_from_int64(1).expect("valid vertex id")
        );
        assert_eq!(
            header.dst,
            VertexId::try_from_int64(2).expect("valid vertex id")
        );
        assert_eq!(header.edge_type, "friend");
        assert_eq!(header.ranking, 3);
        assert_eq!(row[1], Value::BigInt(2024));
    }

    #[test]
    fn flat_vertex_record_row_missing_property_is_null() {
        let record = FlatVertexRecord {
            vid: VertexId::try_from_int64(42).expect("valid vertex id"),
            internal_id: 7,
            tag_name: "person".to_string(),
            props: vec![("age".into(), Value::BigInt(30))],
        };
        let row = make_flat_vertex_record_row(record, &[Arc::from("missing")]);
        assert_eq!(row.len(), 2);
        assert!(matches!(&row[1], Value::Null(_)));
    }

    #[test]
    fn flat_vertex_record_row_tag_name_equals_property_is_null() {
        // The tag-name-yields-map implicit behavior is cancelled: a property
        // name that only equals the tag resolves to Null.
        let record = FlatVertexRecord {
            vid: VertexId::try_from_int64(42).expect("valid vertex id"),
            internal_id: 7,
            tag_name: "person".to_string(),
            props: vec![("age".into(), Value::BigInt(30))],
        };
        let row = make_flat_vertex_record_row(record, &[Arc::from("person")]);
        assert!(matches!(&row[1], Value::Null(_)));
    }
}
