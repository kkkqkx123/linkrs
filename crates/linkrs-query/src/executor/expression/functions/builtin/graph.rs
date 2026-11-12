//! Implementation of graph-related functions
//!
//! Provide functions for manipulating vertices and edges, including id, tags, labels, properties, type, src, dst, and rank.
//! Also includes graph traversal functions: neighbors, degree, shortest_path.

use crate::executor::expression::evaluation_context::graph_storage::GraphStorageRef;
use crate::executor::expression::ExpressionError;
use linkrs_core::Value;

mod pure;
mod storage;

use pure::{execute_id, execute_tags, execute_labels, execute_properties, execute_edge_type, execute_src, execute_dst, execute_rank, execute_startnode, execute_endnode, execute_neighbors, execute_degree, execute_out_edges, execute_in_edges, execute_shortest_path, execute_bfs, execute_connected_components, execute_variable_length_path, execute_pagerank};
use storage::{execute_startnode_with_storage, execute_endnode_with_storage, execute_neighbors_with_storage, execute_degree_with_storage, execute_out_edges_with_storage, execute_in_edges_with_storage, execute_shortest_path_with_storage, execute_bfs_with_storage, execute_connected_components_with_storage, execute_variable_length_path_with_storage, execute_pagerank_with_storage};

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
    Bfs,
    ConnectedComponents,
    VariableLengthPath,
    PageRank,
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
            Self::Bfs => "bfs",
            Self::ConnectedComponents => "connected_components",
            Self::VariableLengthPath => "variable_length_path",
            Self::PageRank => "pagerank",
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
            Self::Bfs => 2,
            Self::ConnectedComponents => 0,
            Self::VariableLengthPath => 3,
            Self::PageRank => 0,
        }
    }

    /// Is it a function with variable parameters?
    pub fn is_variadic(&self) -> bool {
        matches!(self, Self::VariableLengthPath)
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
            Self::OutEdges => "Get all outgoing edges of a vertex",
            Self::InEdges => "Get all incoming edges of a vertex",
            Self::ShortestPath => "Find the shortest path between two vertices (returns path length)",
            Self::Bfs => "Breadth-first search traversal (start_vid, max_depth)",
            Self::ConnectedComponents => "Find all connected components in the graph",
            Self::VariableLengthPath => "Find all paths between two vertices within a depth range (start_vid, end_vid, max_depth) or (start_vid, end_vid, min_depth, max_depth)",
            Self::PageRank => "Calculate PageRank scores for all vertices in the graph",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if !self.is_variadic() && args.len() != self.arity() {
            return Err(ExpressionError::invalid_arity(
                self.name(),
                self.arity(),
                args.len(),
            ));
        }
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
            Self::Bfs => execute_bfs(args),
            Self::ConnectedComponents => execute_connected_components(args),
            Self::VariableLengthPath => execute_variable_length_path(args),
            Self::PageRank => execute_pagerank(args),
        }
    }

    /// Execute with graph storage access.
    /// Falls back to execute() if no results can be obtained from storage.
    pub fn execute_with_storage(
        &self,
        args: &[Value],
        storage: &GraphStorageRef,
    ) -> Result<Value, ExpressionError> {
        if !self.is_variadic() && args.len() != self.arity() {
            return Err(ExpressionError::invalid_arity(
                self.name(),
                self.arity(),
                args.len(),
            ));
        }
        match self {
            Self::StartNode => execute_startnode_with_storage(args, storage),
            Self::EndNode => execute_endnode_with_storage(args, storage),
            Self::Neighbors => execute_neighbors_with_storage(args, storage),
            Self::Degree => execute_degree_with_storage(args, storage),
            Self::OutEdges => execute_out_edges_with_storage(args, storage),
            Self::InEdges => execute_in_edges_with_storage(args, storage),
            Self::ShortestPath => execute_shortest_path_with_storage(args, storage),
            Self::Bfs => execute_bfs_with_storage(args, storage),
            Self::ConnectedComponents => execute_connected_components_with_storage(args, storage),
            Self::VariableLengthPath => execute_variable_length_path_with_storage(args, storage),
            Self::PageRank => execute_pagerank_with_storage(args, storage),
            _ => self.execute(args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
        use linkrs_core::vertex_edge_path::{Edge, Tag, Vertex};
    use std::collections::HashMap;

    fn create_test_vertex() -> Vertex {
        let mut props = HashMap::from([
            ("name".into(), Value::string("Alice")),
            ("age".into(), Value::Int(30)),
        ]);
        props.insert("dept".into(), Value::string("Engineering"));
        let tag = Tag::new("person".into(), props);
        Vertex::new(VertexId::try_from_int64(1).expect("valid vertex id"), tag)
    }

    fn create_test_edge() -> Edge {
        Edge::new(
            VertexId::try_from_int64(1).expect("valid vertex id"),
            VertexId::try_from_int64(2).expect("valid vertex id"),
            "knows".to_string(),
            0,
            HashMap::from([("since".into(), Value::Int(2020))]),
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
            assert_eq!(tags.values.len(), 1);
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
            assert!(props.contains_key(&Value::string("name")));
            assert!(props.contains_key(&Value::string("age")));
            assert!(props.contains_key(&Value::string("dept")));
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
        assert_eq!(result, Value::string("knows"));
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
