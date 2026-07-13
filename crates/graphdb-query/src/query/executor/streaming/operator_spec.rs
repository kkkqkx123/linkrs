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
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        limit: Option<usize>,
        col_names: Vec<String>,
    },
    ScanEdges {
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
    },
    StorageScanEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        limit: Option<usize>,
        edge_type: Option<String>,
        col_names: Vec<String>,
    },
    GetVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
    },
    GetEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
    },
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        direction: String,
    },
    EdgeIndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
    },
    IndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: Option<String>,
        index_value: Option<Value>,
    },
    Argument,
    GetProp {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        edge_ids: Option<Vec<Value>>,
        prop_names: Vec<String>,
    },
    LookupIndex {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
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
    },
    BFSShortest {
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
    },
    AllPaths {
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
    },
    MultiShortestPath {
        target_vertices: Vec<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
    },
}

// ── Sink spec ────────────────────────────────────────────────────────────────

/// Immutable config for sink (data modification) operators.
#[derive(Debug, Clone)]
pub enum SinkSpec {
    InsertVertices {
        vertex_properties: Vec<(String, Expression)>,
        tags: Vec<String>,
    },
    InsertEdges {
        src_col: String,
        dst_col: String,
        edge_type: String,
        edge_properties: Vec<(String, Expression)>,
    },
    UpdateVertices {
        updates: Vec<(String, Expression)>,
    },
    UpdateEdges {
        src_col: String,
        dst_col: String,
        edge_type: String,
        updates: Vec<(String, Expression)>,
    },
    DeleteVertices {
        vertex_id_col: String,
    },
    DeleteEdges {
        src_col: String,
        dst_col: String,
    },
    PipeDeleteVertices {
        vertex_id_col: String,
    },
    PipeDeleteEdges {
        src_col: String,
        dst_col: String,
    },
    DeleteTags {
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
    Concatenate {
        partition_count: usize,
    },
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
        apply_expression: Expression,
    },
    PatternApply {
        pattern: Expression,
    },
}
