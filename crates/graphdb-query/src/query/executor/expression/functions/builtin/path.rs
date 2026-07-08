//! Implementation of path-related functions
//!
//! Provide functions for path operations, including nodes and relationships.

use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

/// Path function enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathFunction {
    Nodes,
    Relationships,
}

impl PathFunction {
    /// Obtain the function name
    pub fn name(&self) -> &str {
        match self {
            Self::Nodes => "nodes",
            Self::Relationships => "relationships",
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
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            Self::Nodes => execute_nodes(args),
            Self::Relationships => execute_relationships(args),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;
    use crate::core::vertex_edge_path::{Edge, Path, Step, Tag, Vertex};
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
}
