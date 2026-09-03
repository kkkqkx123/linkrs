//! Immutable configuration for graph traversal operators.

use graphdb_core::types::expr::Expression;
use graphdb_core::{EdgeDirection, Value};

/// Immutable config for graph traversal operators.
#[derive(Debug, Clone)]
pub enum GraphSpec {
    Expand {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
        col_names: Vec<String>,
    },
    ExpandAll {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
        col_names: Vec<String>,
        src_vids: Vec<Value>,
        step_limit: u32,
        /// When true, the expand operator only counts output rows instead of
        /// materializing them. Used when the downstream is a simple COUNT(*)
        /// aggregate with no GROUP BY or other aggregation functions.
        count_only: bool,
        /// When true, emit `Value::VertexId` / `Value::EdgeId` instead of
        /// full `Value::Vertex(Box)` / `Value::Edge(Box)` in the expand
        /// output.  Eliminates heap allocation for downstream operators that
        /// only need the identifier (e.g. another expand hop, count, join key).
        emit_raw_ids: bool,
        /// When true (always alongside `emit_raw_ids`), the hop's source
        /// column is also emitted as a `Value::VertexId` instead of cloning
        /// the full `Value::Vertex(Box)` carried in from upstream.
        lightweight_source: bool,
    },
    Traverse {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
    },
    BiExpand {
        edge_types: Vec<String>,
        direction: EdgeDirection,
    },
    BiTraverse {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
    },
}
