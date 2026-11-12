use crate::executor::expression::ExpressionError;
use linkrs_core::value::list::List;
use linkrs_core::value::NullType;
use linkrs_core::Value;

pub(crate) fn execute_id(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Vertex(v) => Ok(Value::from(v.vid)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The id function requires a vertex type",
        )),
    }
}

pub(crate) fn execute_tags(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Vertex(v) => {
            let tags: Vec<Value> = vec![Value::string(v.tag.name.clone())];
            Ok(Value::list(List { values: tags }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("tags requires vertex type")),
    }
}

pub(crate) fn execute_labels(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_tags(args)
}

pub(crate) fn execute_properties(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Vertex(v) => {
            let mut props = std::collections::HashMap::new();
            props.extend(
                v.properties()
                    .iter()
                    .map(|(k, v)| (Value::string(k.clone()), v.clone())),
            );
            Ok(Value::map(props))
        }
        Value::Edge(e) => Ok(Value::string_map(e.props.clone())),
        Value::EdgeHeader(_) => Err(ExpressionError::type_error(
            "properties requires a materialized edge: the edge identity reference carries no properties",
        )),
        Value::Map(m) => Ok(Value::map((**m).clone())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "properties requires vertex, edge or map type",
        )),
    }
}

pub(crate) fn execute_edge_type(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Edge(e) => Ok(Value::string(e.edge_type.clone())),
        Value::EdgeHeader(h) => Ok(Value::string(h.edge_type.clone())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The type function requires an edge type",
        )),
    }
}

pub(crate) fn execute_src(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Edge(e) => Ok(Value::from(e.src)),
        Value::EdgeHeader(h) => Ok(Value::from(h.src)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The src function requires the edge type",
        )),
    }
}

pub(crate) fn execute_dst(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Edge(e) => Ok(Value::from(e.dst)),
        Value::EdgeHeader(h) => Ok(Value::from(h.dst)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The dst function requires an edge type",
        )),
    }
}

pub(crate) fn execute_rank(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Edge(e) => Ok(Value::BigInt(e.ranking)),
        Value::EdgeHeader(h) => Ok(Value::BigInt(h.ranking)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The rank function requires an edge type",
        )),
    }
}

pub(crate) fn execute_startnode(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Edge(_) | Value::EdgeHeader(_) => Err(ExpressionError::function_error(
            "startnode() requires graph storage access to resolve the endpoint label; use within a query context",
        )),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The startnode function requires the edge type",
        )),
    }
}

pub(crate) fn execute_endnode(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Edge(_) | Value::EdgeHeader(_) => Err(ExpressionError::function_error(
            "endnode() requires graph storage access to resolve the endpoint label; use within a query context",
        )),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The endnode function requires an edge type",
        )),
    }
}

/// Resolve one edge endpoint to a tagged vertex through the edge type
/// schema. Returns Null when the endpoint vertex no longer exists; errors
/// when the edge type or its endpoint labels are unknown.

pub(crate) fn execute_neighbors(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Vertex(v) => Ok(Value::from(v.vid)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The neighbors function requires a vertex type",
        )),
    }
}

pub(crate) fn execute_degree(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Vertex(v) => {
            let degree = v
                .properties()
                .keys()
                .filter(|k| k.starts_with("neighbor_"))
                .count();
            Ok(Value::BigInt(degree as i64))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The degree function requires a vertex type",
        )),
    }
}

pub(crate) fn execute_out_edges(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Vertex(v) => {
            let edges: Vec<Value> = v
                .properties()
                .iter()
                .filter(|(k, _)| k.starts_with("out_"))
                .map(|(_, v)| v.clone())
                .collect();
            Ok(Value::list(List { values: edges }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The out_edges function requires a vertex type",
        )),
    }
}

pub(crate) fn execute_in_edges(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Vertex(v) => {
            let edges: Vec<Value> = v
                .properties()
                .iter()
                .filter(|(k, _)| k.starts_with("in_"))
                .map(|(_, v)| v.clone())
                .collect();
            Ok(Value::list(List { values: edges }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The in_edges function requires a vertex type",
        )),
    }
}

pub(crate) fn execute_shortest_path(args: &[Value]) -> Result<Value, ExpressionError> {
    let start_vid = match &args[0] {
        Value::BigInt(id) => *id,
        Value::Int(id) => *id as i64,
        _ => {
            return Err(ExpressionError::type_error(
                "shortest_path requires integer vertex IDs",
            ))
        }
    };
    let end_vid = match &args[1] {
        Value::BigInt(id) => *id,
        Value::Int(id) => *id as i64,
        _ => {
            return Err(ExpressionError::type_error(
                "shortest_path requires integer vertex IDs",
            ))
        }
    };
    if start_vid < 0 || end_vid < 0 {
        return Err(ExpressionError::type_error(
            "shortest_path requires non-negative integer vertex IDs",
        ));
    }
    if start_vid == end_vid {
        return Ok(Value::BigInt(0));
    }
    Ok(Value::BigInt(-1))
}

pub(crate) fn execute_bfs(_args: &[Value]) -> Result<Value, ExpressionError> {
    Err(ExpressionError::function_error(
        "bfs() requires graph storage access; use within a query context".to_string(),
    ))
}

pub(crate) fn execute_connected_components(_args: &[Value]) -> Result<Value, ExpressionError> {
    Err(ExpressionError::function_error(
        "connected_components() requires graph storage access; use within a query context"
            .to_string(),
    ))
}

// --- Storage-backed implementations ---

pub(crate) fn execute_variable_length_path(_args: &[Value]) -> Result<Value, ExpressionError> {
    Err(ExpressionError::type_error(
        "variable_length_path() requires graph storage access; use within a query context",
    ))
}

pub(crate) fn execute_pagerank(_args: &[Value]) -> Result<Value, ExpressionError> {
    Err(ExpressionError::function_error(
        "pagerank() requires graph storage access; use within a query context".to_string(),
    ))
}
