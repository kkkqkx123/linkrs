use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::wal::EntityRef;
use crate::core::{Value, Vertex, Edge};
use crate::query::executor::base::{MemoryBudget, MemoryReservation};
use crate::query::executor::streaming::operators::base::OperatorBase;

/// Convert an EntityRef to VertexId for back-to-table fetches.
pub(crate) fn entity_ref_to_vertex_id(entity_ref: &EntityRef) -> Option<VertexId> {
    match entity_ref {
        EntityRef::Vertex(vid) => Some(*vid),
        EntityRef::Edge { .. } => None,
    }
}

pub(crate) fn make_vertex_row(vertex: Vertex) -> Vec<Value> {
    vec![Value::Vertex(Box::new(vertex))]
}

pub(crate) fn make_covering_vertex_row(
    entity_ref: &EntityRef,
    columns: Vec<(String, Value)>,
) -> Option<Vec<Value>> {
    let vertex_id = entity_ref_to_vertex_id(entity_ref)?;
    let properties = columns.into_iter().collect();
    Some(make_vertex_row(
        Vertex::new_with_properties(vertex_id, Vec::new(), properties),
    ))
}

pub(crate) fn make_edge_row(edge: Edge) -> Vec<Value> {
    vec![Value::Edge(Box::new(edge))]
}

pub(crate) fn make_covering_edge_row(
    entity_ref: &EntityRef,
    columns: Vec<(String, Value)>,
    edge_type: String,
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
    Some(make_edge_row(edge))
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
    let Some(runtime) = base.runtime.as_ref() else {
        return Ok(None);
    };
    let bytes = MemoryBudget::estimate_rows_memory(rows);
    runtime.memory_budget.reserve(bytes).map(Some)
}