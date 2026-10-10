//! Immutable configuration for graph traversal operators.

use linkrs_core::types::expr::Expression;
use linkrs_core::{EdgeDirection, Value};

use crate::parser::ast::pattern::PathSemantic;

/// Immutable config for graph traversal operators.
#[derive(Debug, Clone)]
pub enum GraphSpec {
    Expand {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
        col_names: Vec<String>,
        dst_tag: String,
    },
    ExpandAll {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
        col_names: Vec<String>,
        src_vids: Vec<Value>,
        step_limit: u32,
        step_limits: Option<Vec<u32>>,
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
        path_semantic: Option<PathSemantic>,
        dst_tag: String,
        /// Demanded edge properties (`None` means whole edge, empty means
        /// topology only). Drives storage projection pushdown.
        edge_required_props: Option<Vec<String>>,
        /// Demanded destination properties, same encoding as the edge demand.
        dst_required_props: Option<Vec<String>>,
        /// Single-label closed loop proven by the planner syntactically and
        /// re-verified against storage at execution with row fallback.
        closed_loop: bool,
        /// Rowless mode: skip the row view and emit typed columns only. Valid
        /// with empty demands or with bypassable property demands served as
        /// flat `{var}.{prop}` columns, always with a closed loop and a
        /// column-capable consumer.
        skip_rows: bool,
    },
    Traverse {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        path_semantic: Option<PathSemantic>,
        dst_tag: String,
    },
    BiExpand {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        dst_tag: String,
    },
    BiTraverse {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        dst_tag: String,
    },
}
