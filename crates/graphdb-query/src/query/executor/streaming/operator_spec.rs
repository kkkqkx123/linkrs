//! OperatorSpec: Immutable configuration descriptors for operator nodes.
//!
//! Each variant holds only the immutable fields of a corresponding operator
//! — expressions, configuration values, column names — but never cursors,
//! hash tables, buffers, or lifecycle state.  This makes an `OperatorSpec`
//! tree (== [`PhysicalNode`]) suitable for caching, EXPLAIN, and repeated
//! instantiation without shared mutable state.
//!
//! Phase 2 pilot: Source, Filter, Project, Limit, Sort, HashJoin.
//! Remaining operators will be migrated in follow-up phases.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::streaming::executor::SortDirection;
use crate::storage::StorageClient;

// ── Source spec ──────────────────────────────────────────────────────────────

/// Immutable config for source operators.
///
/// Mutable state (`cursor`, `buffer`, `current_index`, `partition_id`,
/// `partition_range`) lives in [`SourceState`](super::operator_state::SourceState).
#[derive(Debug, Clone)]
pub enum SourceSpec {
    ScanVertices {
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
    },
    StorageScanVertices {
        space_name: String,
        limit: Option<usize>,
        col_names: Vec<String>,
    },
    ScanEdges {
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
    },
    StorageScanEdges {
        space_name: String,
        limit: Option<usize>,
        edge_type: Option<String>,
        col_names: Vec<String>,
    },
    GetVertices {
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
    },
    GetEdges {
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
    },
    GetNeighbors {
        space_name: String,
        direction: String,
    },
    EdgeIndexScan {
        space_name: String,
        edge_type: Option<String>,
    },
    IndexScan {
        space_name: String,
        index_name: Option<String>,
        index_value: Option<Value>,
    },
    Argument,
    GetProp {
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        edge_ids: Option<Vec<Value>>,
        prop_names: Vec<String>,
    },
    LookupIndex {
        space_name: String,
        index_name: String,
        index_condition: Option<(String, Value)>,
        limit: Option<usize>,
    },
    Start,
}

// ── Unary spec ───────────────────────────────────────────────────────────────

/// Immutable config for unary (one-input) operators.
#[derive(Debug, Clone)]
pub enum UnarySpec {
    Filter {
        predicate: Expression,
    },
    Project {
        output_expressions: Vec<Expression>,
        output_col_names: Vec<String>,
    },
    Limit {
        offset: u32,
        limit: u32,
    },
    Assign {
        assignments: Vec<(String, Expression)>,
    },
    Remove {
        columns_to_remove: Vec<String>,
    },
    Unwind {
        unwind_column: String,
    },
    AppendVertices {
        vertex_properties: Vec<(String, Expression)>,
    },
    Sample {
        count: u64,
    },
}

// ── Blocking spec ────────────────────────────────────────────────────────────

/// Immutable config for blocking operators.
#[derive(Debug, Clone)]
pub enum BlockingSpec {
    Sort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
    },
    Aggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<(AggregateFunction, Expression)>,
        output_col_names: Vec<String>,
    },
    GroupBy {
        group_by_expressions: Vec<Expression>,
    },
    WindowFunction {
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
    },
    Window {
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
    },
    TopN {
        n: u32,
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
    },
    Distinct,
    Materialize,
    DataCollect,
    RollUpApply {
        rollup_expressions: Vec<Expression>,
    },
    PartialAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<AggregateFunction>,
        output_col_names: Vec<String>,
    },
    FinalAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<AggregateFunction>,
        output_col_names: Vec<String>,
    },
}

// ── Join spec ────────────────────────────────────────────────────────────────

/// Immutable config for binary join operators.
#[derive(Debug, Clone)]
pub enum JoinSpec {
    InnerJoin {
        join_condition: Option<Expression>,
    },
    LeftJoin {
        join_condition: Option<Expression>,
    },
    RightJoin {
        join_condition: Option<Expression>,
    },
    FullOuterJoin {
        join_condition: Option<Expression>,
    },
    CrossJoin,
    SemiJoin {
        join_condition: Option<Expression>,
    },
    HashJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
    },
    HashLeftJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
    },
    NestedLoopJoin {
        join_condition: Option<Expression>,
    },
}

// ── Graph spec ───────────────────────────────────────────────────────────────

/// Immutable config for graph traversal operators.
#[derive(Debug, Clone)]
pub enum GraphSpec {
    Expand {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
    },
    ExpandAll {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
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
    ShortestPath {
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
    BFSShortest {
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        allow_cycles: bool,
        allow_loops: bool,
    },
    AllPaths {
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: usize,
        max_depth: usize,
        acyclic: bool,
        limit: Option<usize>,
        offset: usize,
        filter: Option<Expression>,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
    MultiShortestPath {
        target_vertices: Vec<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        left_vertex_column: String,
        right_vertex_column: String,
        single_shortest: bool,
    },
}

// ── Sink spec ────────────────────────────────────────────────────────────────

/// Immutable config for sink (data modification) operators.
#[derive(Debug, Clone)]
pub enum SinkSpec {
    InsertVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_properties: Vec<(String, Expression)>,
        tags: Vec<String>,
    },
    InsertEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        edge_properties: Vec<(String, Expression)>,
    },
    UpdateVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        updates: Vec<(String, Expression)>,
    },
    UpdateEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        updates: Vec<(String, Expression)>,
    },
    DeleteVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_id_col: String,
    },
    DeleteEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
    },
    PipeDeleteVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_id_col: String,
    },
    PipeDeleteEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
    },
    DeleteTags {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        tag_names: Vec<String>,
        vertex_ids: Option<Vec<Value>>,
    },
}

// ── Exchange spec ────────────────────────────────────────────────────────────

/// Immutable config for exchange (gather / merge / repartition) operators.
///
/// Phase 4: explicit Exchange node replaces ad-hoc Gather coordination.
/// Workers in the query-level `MorselWorkerPool` execute partition tasks
/// dynamically via a shared morsel queue.
#[derive(Debug, Clone)]
pub enum ExchangeSpec {
    /// Concatenate N partition outputs in partition order.
    Concatenate { partition_count: usize },
    /// N-way merge-sort of pre-sorted partition inputs.
    MergeSort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        limit: Option<usize>,
    },
}

// ── Set spec ─────────────────────────────────────────────────────────────────

/// Immutable config for set operators.
#[derive(Debug, Clone)]
pub enum SetSpec {
    Union,
    UnionAll,
    Intersect,
    Except,
    Minus,
}

// ── Apply spec ───────────────────────────────────────────────────────────────

/// Immutable config for apply operators.
#[derive(Debug, Clone)]
pub enum ApplySpec {
    Apply {
        kind: ApplyKind,
        correlated_columns: Vec<String>,
    },
    PatternApply {
        key_expressions: Vec<Expression>,
        anti: bool,
    },
    RollUpApply {
        compare_columns: Vec<String>,
        collect_column: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyKind {
    Standard,
    Semi,
    Anti,
    Single,
    All,
}

// ── DDL spec ─────────────────────────────────────────────────────────────────

/// Immutable config for DDL operators.
#[derive(Debug, Clone)]
pub enum DdlSpec {
    SpaceManage {
        command: crate::query::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode,
    },
    TagManage {
        space_name: String,
        command: crate::query::planning::plan::core::nodes::management::manage_node_enums::TagManageNode,
    },
    EdgeManage {
        space_name: String,
        command: crate::query::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode,
    },
    IndexManage {
        space_name: String,
        command: crate::query::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode,
    },
    DeleteIndex {
        space_name: String,
        index_name: String,
    },
    UserManage {
        command: crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode,
    },
    ShowStats {
        space_name: String,
    },
    Analyze {
        space_name: String,
    },
    Migrate {
        space_name: String,
        action: String,
        migration_data: Option<String>,
    },
}

// ── Fulltext spec ────────────────────────────────────────────────────────────

/// Immutable config for fulltext search operators.
#[derive(Debug, Clone)]
pub enum FulltextSpec {
    FulltextManage {
        space_name: String,
        command: crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode,
    },
    FulltextSearch {
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
    },
    FulltextLookup {
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
    },
    MatchFulltext {
        space_name: String,
        match_expr: Expression,
        match_field: Option<String>,
        tag_name: String,
        field_name: String,
    },
}

// ── Vector spec ──────────────────────────────────────────────────────────────

/// Immutable config for vector search operators.
#[derive(Debug, Clone)]
pub enum VectorSpec {
    VectorManage {
        space_name: String,
        command: crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode,
    },
    VectorSearch {
        space_name: String,
        space_id: u64,
        index_name: String,
        query_vector: Vec<f32>,
        top_k: u32,
        tag_name: String,
        field_name: String,
    },
    VectorLookup {
        space_name: String,
        index_name: String,
        lookup_key: Expression,
    },
    VectorMatch {
        space_name: String,
        pattern: String,
        field: String,
        query_vector: Vec<f32>,
        threshold: Option<f32>,
        tag_name: String,
        field_name: String,
        space_id: u64,
    },
}

// ── Txn spec ─────────────────────────────────────────────────────────────────

/// Immutable config for transaction operators.
#[derive(Debug, Clone)]
pub enum TxnSpec {
    BeginTransaction { transaction_id: Option<String> },
    Commit { transaction_id: Option<String> },
    Rollback { transaction_id: Option<String> },
}
