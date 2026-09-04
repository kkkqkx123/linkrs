//! Implementation of path-related functions
//!
//! Provide functions for path operations, including nodes and relationships.

use crate::executor::expression::ExpressionError;
use graphdb_core::value::list::List;
use graphdb_core::value::NullType;
use graphdb_core::Value;

/// Path function enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathFunction {
    Nodes,
    Relationships,
    Properties,
    IsTrail,
    IsAcyclic,
    PathLength,
}

impl PathFunction {
    /// Obtain the function name
    pub fn name(&self) -> &str {
        match self {
            Self::Nodes => "nodes",
            Self::Relationships => "relationships",
            Self::Properties => "path_properties",
            Self::IsTrail => "is_trail",
            Self::IsAcyclic => "is_acyclic",
            Self::PathLength => "length",
        }
    }

    /// Determine the number of parameters
    pub fn arity(&self) -> usize {
        1
    }

    /// Is it a function with variable parameters?
    pub fn is_variadic(&self) -> bool {
        false
    }

    /// Obtain the function description
    pub fn description(&self) -> &str {
        match self {
            Self::Nodes => "Get all vertices in the path",
            Self::Relationships => "Get all edges in the path",
            Self::Properties => "Get all properties from vertices and edges in the path",
            Self::IsTrail => "Check if path is a trail (no repeated edges)",
            Self::IsAcyclic => "Check if path is acyclic (no repeated vertices)",
            Self::PathLength => "Get the length of a string, path, or list",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            Self::Nodes => execute_nodes(args),
            Self::Relationships => execute_relationships(args),
            Self::Properties => execute_properties(args),
            Self::IsTrail => execute_is_trail(args),
            Self::IsAcyclic => execute_is_acyclic(args),
            Self::PathLength => execute_length(args),
        }
    }
}

fn execute_nodes(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The nodes function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Path(path) => {
            let mut result = vec![Value::Vertex(Box::new((*path.src).clone()))];
            for step in &path.steps {
                result.push(Value::Vertex(Box::new((*step.dst).clone())));
            }
            Ok(Value::list(List { values: result }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("nodes requires a path type")),
    }
}

fn execute_relationships(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "relationships requires 1 argument",
        ));
    }
    match &args[0] {
        Value::Path(path) => {
            let result: Vec<Value> = path
                .steps
                .iter()
                .map(|step| Value::edge((*step.edge).clone()))
                .collect();
            Ok(Value::list(List { values: result }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "relationships requires a path type",
        )),
    }
}

fn execute_properties(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "properties requires 1 argument",
        ));
    }
    match &args[0] {
        Value::Path(path) => {
            let mut all_props = std::collections::HashMap::new();
            // Get properties from source vertex
            for tag in &path.src.tags {
                for (k, v) in &tag.properties {
                    all_props.insert(k.clone(), v.clone());
                }
            }
            for (k, v) in &path.src.properties {
                all_props.insert(k.clone(), v.clone());
            }
            // Get properties from edges and destination vertices
            for step in &path.steps {
                for (k, v) in &step.edge.props {
                    all_props.insert(k.clone(), v.clone());
                }
                for tag in &step.dst.tags {
                    for (k, v) in &tag.properties {
                        all_props.insert(k.clone(), v.clone());
                    }
                }
                for (k, v) in &step.dst.properties {
                    all_props.insert(k.clone(), v.clone());
                }
            }
            let props: Vec<Value> = all_props
                .into_iter()
                .map(|(k, v)| {
                    Value::list(List {
                        values: vec![Value::string(k), v],
                    })
                })
                .collect();
            Ok(Value::list(List { values: props }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "properties requires a path type",
        )),
    }
}

fn execute_is_trail(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("is_trail requires 1 argument"));
    }
    match &args[0] {
        Value::Path(path) => {
            let mut seen_edges = std::collections::HashSet::new();
            for step in &path.steps {
                let edge_id = (
                    step.edge.src.as_int64(),
                    step.edge.dst.as_int64(),
                    &step.edge.edge_type,
                );
                if !seen_edges.insert(edge_id) {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("is_trail requires a path type")),
    }
}

fn execute_is_acyclic(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "is_acyclic requires 1 argument",
        ));
    }
    match &args[0] {
        Value::Path(path) => {
            let mut seen_vertices = std::collections::HashSet::new();
            seen_vertices.insert(path.src.vid.as_int64());
            for step in &path.steps {
                let vid = step.dst.vid.as_int64();
                if !seen_vertices.insert(vid) {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "is_acyclic requires a path type",
        )),
    }
}

fn execute_length(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("length requires 1 argument"));
    }
    match &args[0] {
        Value::Path(path) => Ok(Value::BigInt(path.steps.len() as i64)),
        Value::String(s) => Ok(Value::BigInt(s.len() as i64)),
        Value::List(list) => Ok(Value::BigInt(list.values.len() as i64)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "length requires a string, path, or list type",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::VertexId;
    use graphdb_core::vertex_edge_path::{Edge, Path, Step, Tag, Vertex};
    use std::collections::HashMap;

    fn create_test_vertex_with_id(id: i64) -> Vertex {
        Vertex::new(
            VertexId::from_int64(id),
            vec![Tag::new("person".to_string(), HashMap::new())],
        )
    }

    fn create_test_path() -> Path {
        let v1 = create_test_vertex_with_id(1);
        let v2 = create_test_vertex_with_id(2);
        let v3 = create_test_vertex_with_id(3);

        let e1 = Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "knows".to_string(),
            0,
            HashMap::new(),
        );
        let e2 = Edge::new(
            VertexId::from_int64(2),
            VertexId::from_int64(3),
            "follows".to_string(),
            0,
            HashMap::new(),
        );

        let mut path = Path::new(v1);
        path.add_step(Step {
            dst: Box::new(v2),
            edge: Box::new(e1),
        });
        path.add_step(Step {
            dst: Box::new(v3),
            edge: Box::new(e2),
        });
        path
    }

    #[test]
    fn test_nodes_function() {
        let path = create_test_path();
        let result = PathFunction::Nodes
            .execute(&[Value::Path(Box::new(path))])
            .expect("The execution of the nodes function should succeed");

        if let Value::List(nodes) = result {
            assert_eq!(nodes.values.len(), 3);
            if let Value::Vertex(v) = &nodes.values[0] {
                assert_eq!(v.vid.as_int64(), Some(1));
            } else {
                panic!("The first node should be the vertex.");
            }
            if let Value::Vertex(v) = &nodes.values[1] {
                assert_eq!(v.vid.as_int64(), Some(2));
            } else {
                panic!("The second node should be the vertex.");
            }
            if let Value::Vertex(v) = &nodes.values[2] {
                assert_eq!(v.vid.as_int64(), Some(3));
            } else {
                panic!("The third node should be the vertex.");
            }
        } else {
            panic!("The `nodes` function should return a list.");
        }
    }

    #[test]
    fn test_relationships_function() {
        let path = create_test_path();
        let result = PathFunction::Relationships
            .execute(&[Value::Path(Box::new(path))])
            .expect("The relationships function should execute successfully");

        if let Value::List(edges) = result {
            assert_eq!(edges.values.len(), 2);
            if let Value::Edge(e) = &edges.values[0] {
                assert_eq!(e.edge_type, "knows");
            } else {
                panic!("The first element should be the edge.");
            }
            if let Value::Edge(e) = &edges.values[1] {
                assert_eq!(e.edge_type, "follows");
            } else {
                panic!("The second element should be the edge.");
            }
        } else {
            panic!("The `relationships` function should return a list.");
        }
    }

    #[test]
    fn test_nodes_empty_path() {
        let v1 = create_test_vertex_with_id(1);
        let path = Path::new(v1);
        let result = PathFunction::Nodes
            .execute(&[Value::Path(Box::new(path))])
            .expect("The execution of the nodes function should succeed");

        if let Value::List(nodes) = result {
            assert_eq!(nodes.values.len(), 1);
        } else {
            panic!("The `nodes` function should return a list.");
        }
    }

    #[test]
    fn test_relationships_empty_path() {
        let v1 = create_test_vertex_with_id(1);
        let path = Path::new(v1);
        let result = PathFunction::Relationships
            .execute(&[Value::Path(Box::new(path))])
            .expect("The relationships function should execute successfully");

        if let Value::List(edges) = result {
            assert_eq!(edges.values.len(), 0);
        } else {
            panic!("The `relationships` function should return a list.");
        }
    }

    #[test]
    fn test_null_handling() {
        let null_value = Value::Null(NullType::Null);

        assert_eq!(
            PathFunction::Nodes
                .execute(std::slice::from_ref(&null_value))
                .expect("The nodes function should handle NULL"),
            Value::Null(NullType::Null)
        );
        assert_eq!(
            PathFunction::Relationships
                .execute(std::slice::from_ref(&null_value))
                .expect("The relationshipships function should handle NULL."),
            Value::Null(NullType::Null)
        );
    }

    #[test]
    fn test_is_trail() {
        let path = create_test_path();
        let result = PathFunction::IsTrail
            .execute(&[Value::Path(Box::new(path))])
            .expect("is_trail should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_is_acyclic() {
        let path = create_test_path();
        let result = PathFunction::IsAcyclic
            .execute(&[Value::Path(Box::new(path))])
            .expect("is_acyclic should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_path_length() {
        let path = create_test_path();
        let result = PathFunction::PathLength
            .execute(&[Value::Path(Box::new(path))])
            .expect("length should succeed");
        assert_eq!(result, Value::BigInt(2));
    }

    #[test]
    fn test_path_length_string() {
        let result = PathFunction::PathLength
            .execute(&[Value::string("hello")])
            .expect("length should succeed");
        assert_eq!(result, Value::BigInt(5));
    }

    #[test]
    fn test_path_length_list() {
        let list = Value::list(List {
            values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
        });
        let result = PathFunction::PathLength
            .execute(&[list])
            .expect("length should succeed");
        assert_eq!(result, Value::BigInt(3));
    }
}
