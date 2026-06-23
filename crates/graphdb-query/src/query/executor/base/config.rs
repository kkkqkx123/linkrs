//! Actuator Configuration Structure
//!
//! This module defines configuration constructs for the various actuators, which are used to reduce the number of arguments to the constructors

use std::sync::Arc;

use crate::core::types::VertexId;
use crate::core::Expression;
use crate::query::validator::context::ExpressionAnalysisContext;
use parking_lot::RwLock;

/// Universal Actuator Configuration
///
/// Encapsulates the basic configuration common to all actuators
pub struct ExecutorConfig<S> {
    pub id: i64,
    pub storage: Arc<RwLock<S>>,
    pub expr_context: Arc<ExpressionAnalysisContext>,
}

impl<S> ExecutorConfig<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            id,
            storage,
            expr_context,
        }
    }
}

/// Index Scanning Actuator Configuration
pub struct IndexScanConfig {
    pub space_id: u64,
    pub tag_id: i32,
    pub index_id: i32,
    pub index_name: String,
    pub schema_name: String,
    pub scan_type: String,
    pub scan_limits: Vec<crate::query::planning::plan::core::nodes::access::IndexLimit>,
    pub filter: Option<Expression>,
    pub return_columns: Vec<String>,
    pub limit: Option<usize>,
    pub is_edge: bool,
}

/// Path Actuator Configuration
pub struct PathConfig {
    pub start_vertex: crate::core::Value,
    pub end_vertex: Option<crate::core::Value>,
    pub max_hops: usize,
    pub edge_types: Option<Vec<String>>,
    pub direction: crate::core::types::EdgeDirection,
}

/// BFS Shortest Path Algorithm Configuration
pub struct BfsShortestConfig {
    pub steps: usize,
    pub direction: crate::core::types::EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub space_name: String,
}

/// Multiple Starting Point Shortest Path Configuration
pub struct MultiShortestPathConfig {
    pub start_vids: Vec<VertexId>,
    pub direction: crate::core::types::EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub max_steps: usize,
    pub space_name: String,
}

/// All path configurations
pub struct AllPathsConfig {
    pub left_start_ids: Vec<VertexId>,
    pub right_start_ids: Vec<VertexId>,
    pub max_hops: usize,
    pub edge_types: Option<Vec<String>>,
    pub direction: crate::core::types::EdgeDirection,
    pub space_name: String,
}

/// Shortest Path Configuration
pub struct ShortestPathConfig {
    pub start_vertex_ids: Vec<VertexId>,
    pub direction: crate::core::types::EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub space_name: String,
}

/// Connected Actuator Configuration
pub struct JoinConfig {
    pub left_var: String,
    pub right_var: String,
    pub hash_keys: Vec<Expression>,
    pub probe_keys: Vec<Expression>,
    pub col_names: Vec<String>,
}

/// Connected actuator configuration with description
pub struct JoinConfigWithDesc {
    pub left_var: String,
    pub right_var: String,
    pub hash_keys: Vec<Expression>,
    pub probe_keys: Vec<Expression>,
    pub col_names: Vec<String>,
    pub description: String,
}

/// Cyclic actuator configuration
pub struct LoopConfig {
    pub loop_var: String,
    pub loop_condition: Expression,
}

/// Additional Vertex Actuator Configurations
pub struct AppendVerticesConfig {
    pub input_var: String,
    pub src_expression: Expression,
    pub v_filter: Option<Expression>,
    pub col_names: Vec<String>,
    pub dedup: bool,
    pub need_fetch_prop: bool,
}

/// Mode Application Actuator Configuration
pub struct PatternApplyConfig {
    pub left_input_var: String,
    pub right_input_var: String,
    pub key_cols: Vec<Expression>,
    pub col_names: Vec<String>,
    pub is_anti_predicate: bool,
}

/// Summarize application actuator configurations
pub struct RollupApplyConfig {
    pub left_input_var: String,
    pub right_input_var: String,
    pub compare_cols: Vec<Expression>,
    pub collect_col: Expression,
    pub col_names: Vec<String>,
}
