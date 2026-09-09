//! Immutable configuration for recursive fragment operators.

use std::sync::Arc;

use graphdb_core::{EdgeDirection, Value};

use crate::executor::streaming::plan::types::PhysicalPlan;

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
    /// Recursive-CTE fixpoint (`WITH [RECURSIVE] name AS (anchor [UNION ALL step])`).
    ///
    /// The anchor sub-plan runs once; the step sub-plan re-runs with the
    /// previous delta published as the CTE working table (read through
    /// [`CteScan`](super::source::SourceSpec::CteScan) sources keyed by the
    /// mangled `cte_name`) until no new rows appear or `max_iterations` is
    /// reached. `step` is `None` for a non-recursive CTE (inline view).
    Fixpoint {
        /// Mangled CTE tag (`crate::cte::mangle_cte_name`).
        cte_name: String,
        /// Anchor sub-plan (runs once, seeds the working table).
        anchor: Arc<PhysicalPlan>,
        /// Step sub-plan (re-runs per iteration over the delta).
        step: Option<Arc<PhysicalPlan>>,
        /// Fixpoint iteration cap (errors when exceeded).
        max_iterations: u64,
        /// Output column names (single column in V1).
        col_names: Vec<String>,
    },
}
