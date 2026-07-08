//! Test data generation module
//!
//! Provide various test data generation functions

#![allow(dead_code)]

use graphdb_query::core::types::VertexId;
use graphdb_query::core::vertex_edge_path::{Edge, Tag, Vertex};
use graphdb_query::core::Value;
use std::collections::HashMap;

/// Create simple vertices (with only one label)
pub fn create_simple_vertex(vid: i64, _tag_name: &str, name: &str, age: i64) -> Vertex {
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::String(name.to_string()));
    props.insert("age".to_string(), Value::Int(age as i32));
    let tag = Tag::new("Person".to_string(), props);
    create_vertex(VertexId::from_int64(vid), vec![tag])
}

/// create a vertex
pub fn create_vertex(vid: VertexId, tags: Vec<Tag>) -> Vertex {
    Vertex::new(vid, tags)
}

/// create an edge
pub fn create_edge(src: VertexId, dst: VertexId, edge_type: &str) -> Edge {
    Edge::new(src, dst, edge_type.to_string(), 0, HashMap::new())
}

/// Social networks test dataset
/// Return (vertex list, edge list)
pub fn social_network_dataset() -> (Vec<Vertex>, Vec<Edge>) {
    // Create 4 Person Vertices
    let vertices = vec![
        create_simple_vertex(1, "Person", "Alice", 30),
        create_simple_vertex(2, "Person", "Bob", 25),
        create_simple_vertex(3, "Person", "Charlie", 35),
        create_simple_vertex(4, "Person", "David", 28),
    ];

    // Create a KNOWS relationship edge
    let edges = vec![
        create_edge(VertexId::from_int64(1), VertexId::from_int64(2), "KNOWS"),
        create_edge(VertexId::from_int64(1), VertexId::from_int64(3), "KNOWS"),
        create_edge(VertexId::from_int64(2), VertexId::from_int64(3), "KNOWS"),
        create_edge(VertexId::from_int64(3), VertexId::from_int64(4), "KNOWS"),
    ];

    (vertices, edges)
}
