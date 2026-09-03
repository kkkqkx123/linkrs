//! Immutable configuration for recursive fragment operators.

use graphdb_core::{EdgeDirection, Value};

/// Immutable config for recursive fragment operators.
///
/// Variable-length path traversal, BFS, shortest-path, and multi-round
/// graph algorithms use this explicit recursive fragment spec.
///
/// Frontier, visited-set, path-predecessor, and result-queue allocations
/// are all accounted against the query memory pool.  Each round and
/// batch-expansion checks the cancellation token.
#[derive(Debug, Clone)]
pub enum RecursiveFragmentSpec {
    /// Bidirectional BFS shortest path between start and target vertices.
    ShortestPath {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
    /// Multi-source multi-target shortest path via bidirectional BFS.
    MultiShortestPath {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        left_vertex_column: String,
        right_vertex_column: String,
        single_shortest: bool,
    },
    /// BFS traversal with configurable depth and cycle policies.
    BFSShortest {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        allow_loops: bool,
    },
    /// Enumerate all paths between start and target vertices.
    AllPaths {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: usize,
        max_depth: usize,
        acyclic: bool,
        limit: Option<usize>,
        offset: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
}
