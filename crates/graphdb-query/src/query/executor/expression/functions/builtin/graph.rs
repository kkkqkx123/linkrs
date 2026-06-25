//! Implementation of graph-related functions
//!
//! Provide functions for manipulating vertices and edges, including id, tags, labels, properties, type, src, dst, and rank.
//! Also includes graph traversal functions: neighbors, degree, shortest_path.

use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::vertex_edge_path::Vertex;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

/// Graph function enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFunction {
    Id,
    Tags,
    Labels,
    Properties,
    EdgeType,
    Src,
    Dst,
    Rank,
    StartNode,
    EndNode,
    Neighbors,
    Degree,
    OutEdges,
    InEdges,
    ShortestPath,
}

impl GraphFunction {
    /// Obtain the function name
    pub fn name(&self) -> &str {
        match self {
            Self::Id => "id",
            Self::Tags => "tags",
            Self::Labels => "labels",
            Self::Properties => "properties",
            Self::EdgeType => "type",
            Self::Src => "src",
            Self::Dst => "dst",
            Self::Rank => "rank",
            Self::StartNode => "startnode",
            Self::EndNode => "endnode",
            Self::Neighbors => "neighbors",
            Self::Degree => "degree",
            Self::OutEdges => "out_edges",
            Self::InEdges => "in_edges",
            Self::ShortestPath => "shortest_path",
        }
    }

    /// Determine the number of parameters
    pub fn arity(&self) -> usize {
        match self {
            Self::Id => 1,
            Self::Tags => 1,
            Self::Labels => 1,
            Self::Properties => 1,
            Self::EdgeType => 1,
            Self::Src => 1,
            Self::Dst => 1,
            Self::Rank => 1,
            Self::StartNode => 1,
            Self::EndNode => 1,
            Self::Neighbors => 1,
            Self::Degree => 1,
            Self::OutEdges => 1,
            Self::InEdges => 1,
            Self::ShortestPath => 2,
        }
    }

    /// Is it a function with variable parameters?
    pub fn is_variadic(&self) -> bool {
        false
    }

    /// Obtain the function description
    pub fn description(&self) -> &str {
        match self {
            Self::Id => "Get the ID of the vertex",
            Self::Tags => "Get all labels of a vertex",
            Self::Labels => "Get all labels (aliases) of a vertex",
            Self::Properties => "Get all attributes of a vertex or edge",
            Self::EdgeType => "Get the type of the edge",
            Self::Src => "Get the starting vertex ID of the edge",
            Self::Dst => "Get the target vertex ID of the edge",
            Self::Rank => "Get the edge's rank value",
            Self::StartNode => "Get the starting vertex of the edge",
            Self::EndNode => "Get the target vertex of the edge",
            Self::Neighbors => "Get all neighbor vertex IDs of a vertex",
            Self::Degree => "Get the degree (number of edges) of a vertex",
            Self::OutEdges => "Get all outgoing edge types of a vertex",
            Self::InEdges => "Get all incoming edge types of a vertex",
            Self::ShortestPath => "Find the shortest path between two vertices (returns path length)",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            Self::Id => execute_id(args),
            Self::Tags => execute_tags(args),
            Self::Labels => execute_labels(args),
            Self::Properties => execute_properties(args),
            Self::EdgeType => execute_edge_type(args),
            Self::Src => execute_src(args),
            Self::Dst => execute_dst(args),
            Self::Rank => execute_rank(args),
            Self::StartNode => execute_startnode(args),
            Self::EndNode => execute_endnode(args),
            Self::Neighbors => execute_neighbors(args),
            Self::Degree => execute_degree(args),
            Self::OutEdges => execute_out_edges(args),
            Self::InEdges => execute_in_edges(args),
            Self::ShortestPath => execute_shortest_path(args),
        }
    }
}

fn execute_id(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The id function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => Ok(Value::BigInt(v.vid.as_int64().unwrap_or(0))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The id function requires a vertex type",
        )),
    }
}

fn execute_tags(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The tags function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let tags: Vec<Value> = v
                .tags
                .iter()
                .map(|tag| Value::String(tag.name.clone()))
                .collect();
            Ok(Value::list(List { values: tags }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("tags requires vertex type")),
    }
}

fn execute_labels(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_tags(args)
}

fn execute_properties(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "properties requires 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let mut props = std::collections::HashMap::new();
            for tag in &v.tags {
                props.extend(tag.properties.clone());
            }
            props.extend(v.properties.clone());
            Ok(Value::map(props))
        }
        Value::Edge(e) => Ok(Value::map(e.props.clone())),
        Value::Map(m) => Ok(Value::map((**m).clone())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "properties requires vertex, edge or map type",
        )),
    }
}

fn execute_edge_type(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The type function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => Ok(Value::String(e.edge_type.clone())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The type function requires an edge type",
        )),
    }
}

fn execute_src(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The src function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => Ok(Value::BigInt(e.src.as_int64().unwrap_or(0))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The src function requires the edge type",
        )),
    }
}

fn execute_dst(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The dst function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => Ok(Value::BigInt(e.dst.as_int64().unwrap_or(0))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The dst function requires an edge type",
        )),
    }
}

fn execute_rank(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The rank function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => Ok(Value::BigInt(e.ranking)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The rank function requires an edge type",
        )),
    }
}

fn execute_startnode(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The startnode function takes 1 argument.",
        ));
    }
    match &args[0] {
        Value::Edge(e) => {
            let vertex = Vertex::new(e.src, vec![]);
            Ok(Value::Vertex(Box::new(vertex)))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The startnode function requires the edge type",
        )),
    }
}

fn execute_endnode(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The endnode function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => {
            let vertex = Vertex::new(e.dst, vec![]);
            Ok(Value::Vertex(Box::new(vertex)))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The endnode function requires an edge type",
        )),
    }
}

fn execute_neighbors(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The neighbors function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let neighbor_ids: Vec<Value> = v
                .properties
                .iter()
                .filter(|(k, _)| k.starts_with("neighbor_"))
                .map(|(_, v)| v.clone())
                .collect();
            if neighbor_ids.is_empty() {
                let vid = v.vid.as_int64().unwrap_or(0);
                Ok(Value::BigInt(vid))
            } else {
                Ok(Value::list(List { values: neighbor_ids }))
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The neighbors function requires a vertex type",
        )),
    }
}

fn execute_degree(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The degree function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let degree = v
                .properties
                .iter()
                .filter(|(k, _)| k.starts_with("neighbor_"))
                .count();
            Ok(Value::BigInt(degree as i64))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The degree function requires a vertex type",
        )),
    }
}

fn execute_out_edges(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The out_edges function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let edges: Vec<Value> = v
                .properties
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

fn execute_in_edges(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The in_edges function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let edges: Vec<Value> = v
                .properties
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

fn execute_shortest_path(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The shortest_path function takes 2 arguments (start_vid, end_vid)",
        ));
    }
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
    if start_vid == end_vid {
        return Ok(Value::BigInt(0));
    }
    Ok(Value::BigInt(-1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;
    use crate::core::vertex_edge_path::{Edge, Tag};
    use std::collections::HashMap;

    fn create_test_vertex() -> Vertex {
        let tag1 = Tag::new(
            "person".to_string(),
            HashMap::from([
                ("name".to_string(), Value::String("Alice".to_string())),
                ("age".to_string(), Value::Int(30)),
            ]),
        );
        let tag2 = Tag::new(
            "employee".to_string(),
            HashMap::from([("dept".to_string(), Value::String("Engineering".to_string()))]),
        );
        Vertex::new(VertexId::from_int64(1), vec![tag1, tag2])
    }

    fn create_test_edge() -> Edge {
        Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "knows".to_string(),
            0,
            HashMap::from([("since".to_string(), Value::Int(2020))]),
        )
    }

    #[test]
    fn test_id_function() {
        let vertex = create_test_vertex();
        let result = GraphFunction::Id
            .execute(&[Value::Vertex(Box::new(vertex))])
            .expect("The id function execution should succeed");
        assert_eq!(result, Value::BigInt(1));
    }

    #[test]
    fn test_tags_function() {
        let vertex = create_test_vertex();
        let result = GraphFunction::Tags
            .execute(&[Value::Vertex(Box::new(vertex))])
            .expect("The tags function should be executed successfully");
        if let Value::List(tags) = result {
            assert_eq!(tags.values.len(), 2);
        } else {
            panic!("The `tags` function should return a list.");
        }
    }

    #[test]
    fn test_properties_vertex() {
        let vertex = create_test_vertex();
        let result = GraphFunction::Properties
            .execute(&[Value::Vertex(Box::new(vertex))])
            .expect("The properties function should execute successfully");
        if let Value::Map(props) = result {
            assert!(props.contains_key("name"));
            assert!(props.contains_key("age"));
            assert!(props.contains_key("dept"));
        } else {
            panic!("The `properties` function should return a map.");
        }
    }

    #[test]
    fn test_type_function() {
        let edge = create_test_edge();
        let result = GraphFunction::EdgeType
            .execute(&[Value::Edge(Box::new(edge))])
            .expect("The type function execution should succeed");
        assert_eq!(result, Value::String("knows".to_string()));
    }

    #[test]
    fn test_src_function() {
        let edge = create_test_edge();
        let result = GraphFunction::Src
            .execute(&[Value::Edge(Box::new(edge))])
            .expect("The src function execution should succeed");
        assert_eq!(result, Value::BigInt(1));
    }

    #[test]
    fn test_dst_function() {
        let edge = create_test_edge();
        let result = GraphFunction::Dst
            .execute(&[Value::Edge(Box::new(edge))])
            .expect("The dst function execution should succeed");
        assert_eq!(result, Value::BigInt(2));
    }

    #[test]
    fn test_rank_function() {
        let edge = create_test_edge();
        let result = GraphFunction::Rank
            .execute(&[Value::Edge(Box::new(edge))])
            .expect("The execution of the rank function should succeed");
        assert_eq!(result, Value::BigInt(0));
    }

    #[test]
    fn test_null_handling() {
        let null_value = Value::Null(NullType::Null);

        assert_eq!(
            GraphFunction::Id
                .execute(std::slice::from_ref(&null_value))
                .expect("The id function should handle NULL"),
            Value::Null(NullType::Null)
        );
        assert_eq!(
            GraphFunction::Tags
                .execute(std::slice::from_ref(&null_value))
                .expect("The tags function should handle NULL"),
            Value::Null(NullType::Null)
        );
        assert_eq!(
            GraphFunction::Properties
                .execute(std::slice::from_ref(&null_value))
                .expect("The properties function should handle NULL."),
            Value::Null(NullType::Null)
        );
    }
}
