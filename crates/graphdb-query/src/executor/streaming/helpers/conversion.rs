//! Data type conversions for streaming executor
//!
//! Converts between Vertex/Edge and row representations.

use graphdb_core::Value;
use graphdb_core::{Edge, Vertex};

/// Render a vertex id as its raw external string without display quoting.
fn vid_raw_string(vid: &Vertex) -> String {
    if let Some(s) = vid.vid.as_str() {
        s.to_string()
    } else if let Some(i) = vid.vid.as_int64() {
        i.to_string()
    } else if let Some(u) = vid.vid.as_u64() {
        u.to_string()
    } else {
        format!("{:?}", vid.vid.as_bytes())
    }
}

fn edge_raw_string(vid: &graphdb_core::types::storage_ids::VertexId) -> String {
    if let Some(s) = vid.as_str() {
        s.to_string()
    } else if let Some(i) = vid.as_int64() {
        i.to_string()
    } else if let Some(u) = vid.as_u64() {
        u.to_string()
    } else {
        format!("{:?}", vid.as_bytes())
    }
}

/// Convert a Vertex to row representation
pub fn vertex_to_row(vertex: &Vertex) -> Vec<Value> {
    let mut row = vec![
        Value::BigInt(vertex.id),
        Value::string(vid_raw_string(vertex)),
    ];

    // Add tags
    for tag in &vertex.tags {
        row.push(Value::string(tag.name.clone()));
    }

    // Add first 3 properties from the unified tag-first view so tag-only
    // rows are not silently dropped.
    for value in vertex.get_all_properties().into_values().take(3) {
        row.push((*value).clone());
    }

    row
}

/// Convert an Edge to row representation
pub fn edge_to_row(edge: &Edge) -> Vec<Value> {
    let mut row = vec![
        Value::string(edge_raw_string(&edge.src)),
        Value::string(edge_raw_string(&edge.dst)),
        Value::string(edge.edge_type.clone()),
        Value::BigInt(edge.ranking),
    ];

    // Add first 2 properties (simplified)
    for value in edge.props.values().take(2) {
        row.push(value.clone());
    }

    row
}

/// Convert Vertex collection to rows, filtering by partition range.
/// Range uses `i64` to match the real vertex ID type and avoid silent
/// truncation of values >= 2^32 or negative IDs.
pub fn vertices_to_rows(
    vertices: Vec<Vertex>,
    partition_range: &std::ops::Range<i64>,
) -> Vec<Vec<Value>> {
    vertices
        .into_iter()
        .filter(|v| v.id >= partition_range.start && v.id < partition_range.end)
        .map(|v| vertex_to_row(&v))
        .collect()
}

/// Convert Edge collection to rows, filtering by partition range.
/// Only edges whose source ID can be parsed as `i64` are matched against
/// the range; non-numeric source IDs are excluded.
pub fn edges_to_rows(edges: Vec<Edge>, partition_range: &std::ops::Range<i64>) -> Vec<Vec<Value>> {
    edges
        .into_iter()
        .filter(|e| {
            edge_raw_string(&e.src)
                .parse::<i64>()
                .is_ok_and(|id| id >= partition_range.start && id < partition_range.end)
        })
        .map(|e| edge_to_row(&e))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::storage_ids::VertexId;

    #[test]
    fn test_vertex_conversion() {
        let vertex = Vertex {
            id: 123,
            vid: VertexId::from_string("vertex_123"),
            tags: vec![],
            properties: std::collections::HashMap::new(),
        };

        let row = vertex_to_row(&vertex);

        // Verify row structure: id and vid form the base layout.
        assert_eq!(row.len(), 2);
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

        // Verify row structure: src, dst, edge_type, ranking.
        assert_eq!(row.len(), 4);
        assert_eq!(row[2], Value::string("follows"));
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
        let partition_range = std::ops::Range {
            start: 15i64,
            end: 35,
        };
        let rows = vertices_to_rows(vertices, &partition_range);

        // Should include vertices with id 20 and 30, but not 10
        assert_eq!(rows.len(), 2);
    }
}
