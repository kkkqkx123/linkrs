//! Data type conversions for streaming executor
//!
//! Converts between Vertex/Edge and row representations.

use crate::core::value::NullType;
use crate::core::Value;
use crate::core::{Edge, Vertex};
use std::ops::Range;

/// Convert a Vertex to row representation
pub fn vertex_to_row(vertex: &Vertex) -> Vec<Value> {
    let mut row = vec![
        Value::BigInt(vertex.id),
        Value::String(vertex.vid.to_string()),
    ];

    // Add tags
    for tag in &vertex.tags {
        row.push(Value::String(tag.name.clone()));
    }

    // Add first 3 properties (simplified)
    let mut prop_count = 0;
    for (_key, value) in &vertex.properties {
        if prop_count >= 3 {
            break;
        }
        row.push(value.clone());
        prop_count += 1;
    }

    // Ensure we have at least 5 columns for compatibility
    while row.len() < 5 {
        row.push(Value::Null(NullType::Null));
    }

    row
}

/// Convert an Edge to row representation
pub fn edge_to_row(edge: &Edge) -> Vec<Value> {
    let mut row = vec![
        Value::String(edge.src.to_string()),
        Value::String(edge.dst.to_string()),
        Value::String(edge.edge_type.clone()),
        Value::BigInt(edge.ranking),
    ];

    // Add first 2 properties (simplified)
    let mut prop_count = 0;
    for (_key, value) in &edge.props {
        if prop_count >= 2 {
            break;
        }
        row.push(value.clone());
        prop_count += 1;
    }

    // Ensure we have at least 5 columns for compatibility
    while row.len() < 5 {
        row.push(Value::Null(NullType::Null));
    }

    row
}

/// Convert Vertex collection to rows, filtering by partition range
pub fn vertices_to_rows(vertices: Vec<Vertex>, partition_range: &Range<u32>) -> Vec<Vec<Value>> {
    vertices
        .into_iter()
        // Filter by partition range using vertex id
        .filter(|v| {
            let vid = v.id as u32;
            vid >= partition_range.start && vid < partition_range.end
        })
        .map(|v| vertex_to_row(&v))
        .collect()
}

/// Convert Edge collection to rows, filtering by partition range
pub fn edges_to_rows(edges: Vec<Edge>, partition_range: &Range<u32>) -> Vec<Vec<Value>> {
    edges
        .into_iter()
        // Filter by partition range using source vertex id
        .filter(|e| {
            let src_id = e.src.to_string().parse::<u32>().unwrap_or(0);
            src_id >= partition_range.start && src_id < partition_range.end
        })
        .map(|e| edge_to_row(&e))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::VertexId;

    #[test]
    fn test_vertex_conversion() {
        let vertex = Vertex {
            id: 123,
            vid: VertexId::from_string("vertex_123"),
            tags: vec![],
            properties: std::collections::HashMap::new(),
        };

        let row = vertex_to_row(&vertex);

        // Verify row structure: id, vid, + at least 3 null properties
        assert!(row.len() >= 5);
        assert_eq!(row[0], Value::BigInt(123));
    }

    #[test]
    fn test_edge_conversion() {
        let edge = Edge {
            src: VertexId::from_string("src_1"),
            dst: VertexId::from_string("dst_2"),
            edge_type: "follows".to_string(),
            ranking: 42,
            props: std::collections::HashMap::new(),
        };

        let row = edge_to_row(&edge);

        // Verify row structure: src, dst, edge_type, ranking, + at least 1 property
        assert!(row.len() >= 5);
        assert_eq!(row[2], Value::String("follows".to_string()));
        assert_eq!(row[3], Value::BigInt(42));
    }

    #[test]
    fn test_partition_filtering() {
        let vertices = vec![
            Vertex {
                id: 10,
                vid: VertexId::from_string("v10"),
                tags: vec![],
                properties: std::collections::HashMap::new(),
            },
            Vertex {
                id: 20,
                vid: VertexId::from_string("v20"),
                tags: vec![],
                properties: std::collections::HashMap::new(),
            },
            Vertex {
                id: 30,
                vid: VertexId::from_string("v30"),
                tags: vec![],
                properties: std::collections::HashMap::new(),
            },
        ];

        // Filter for partition range [15, 35)
        let partition_range = Range { start: 15, end: 35 };
        let rows = vertices_to_rows(vertices, &partition_range);

        // Should include vertices with id 20 and 30, but not 10
        assert_eq!(rows.len(), 2);
    }
}
