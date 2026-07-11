//! StreamingExecutor: Enum-based pull executor with modular operator implementation
//!
//! This file contains:
//! - StreamingExecutor enum definition (all 79 operator variants)
//! - SortDirection enum
//! - Coordination methods (open, next, stop, close) that dispatch to operator modules
//!
//! Operator implementations are in submodules:
//! - context - Expression evaluation context
//! - operators/ - Operator implementations (sources, single_input, stateful, binary, set_ops)
//! - helpers/ - Helper functions (comparison, aggregation, conversion)
//!
//! Note: All 79+ operator variants are fully implemented (no stubs).

use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

use super::chunk::DataChunk;
use super::driver;
use super::runtime::ExecutionRuntime;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::Value;
use crate::query::executor::base::{MemoryTracker, Spillable};
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::cursor::{EdgeCursor, VertexCursor};
use crate::storage::StorageClient;
#[cfg(feature = "qdrant")]
use crate::sync::VectorSyncCoordinator;

pub mod context;
pub mod helpers;
pub mod operators;

pub use context::ValueRowContext;
pub use helpers::{aggregation, comparison, conversion};

/// Sort direction for ORDER BY clause
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Phase for FullOuterJoin execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullOuterJoinPhase {
    BuildingRight,
    ProbeLeft,
    EmitUnmatchedRight,
}

/// Pull-based streaming executor
///
/// Each variant handles different operation types (79 total):
/// - Data sources (8): ScanVertices, ScanEdges, GetVertices, GetEdges, GetNeighbors, IndexScan, Sample, Argument
/// - Single input (6): Filter, Project, Limit, Distinct, Window, Dedup
/// - Stateful (4): Aggregate, Sort, GroupBy, WindowFunction
/// - Binary input (8): HashJoin, NestedLoopJoin, InnerJoin, LeftJoin, RightJoin, FullOuterJoin, CrossJoin, SemiJoin
/// - Set operations (4): Union, UnionAll, Intersect, Except, Minus
/// - Graph traversal (11): Expand, Traverse, AppendVertices, BiExpand, BiTraverse, ShortestPath, BFSShortest, AllPaths, MultiShortestPath, ExpandAll, TraverseAll
/// - Data modification (8): InsertVertices, InsertEdges, UpdateVertices, UpdateEdges, DeleteVertices, DeleteEdges, PipeDeleteVertices, PipeDeleteEdges
/// - Search operations (5): FulltextSearch, FulltextLookup, MatchFulltext, VectorSearch, VectorLookup
/// - Management/DDL (7): SpaceManage, TagManage, EdgeManage, IndexManage, UserManage, FulltextManage, VectorManage
/// - Other operations (14): Materialize, Remove, Assign, Apply, PatternApply, RollUpApply, DataCollect, Unwind, TopN, Loop, Select, PassThrough, BeginTransaction, Commit, Rollback, ShowStats
///
/// Note: Variants marked as stubs will return "not yet implemented" error until actual implementations are added.
#[derive(Debug)]
pub enum StreamingExecutor {
    // ============ Data Sources ============
    /// Scan vertices from a partition
    /// Input data is pre-loaded into buffer (from storage layer)
    ScanVertices {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        /// Column names from planner (avoids col_N inference)
        col_names: Vec<String>,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Scan vertices from storage on first pull.
    ///
    /// Holds a live cursor and pulls batches on each `next()` call.
    /// When the underlying storage provides a native lazy cursor this
    /// translates to truly on-demand IO; currently uses the Vec-backed
    /// cursor wrapper as a bridge.
    StorageScanVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        limit: Option<usize>,
        cursor: Option<Box<dyn VertexCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Scan edges from a partition
    /// Input data is pre-loaded into buffer (from storage layer)
    ScanEdges {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        /// Column names from planner (avoids col_N inference)
        col_names: Vec<String>,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Scan edges from storage on first pull.
    ///
    /// Holds a live cursor and pulls batches on each `next()` call.
    StorageScanEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        limit: Option<usize>,
        /// Optional edge type filter (Phase 3: now passed from plan node).
        edge_type: Option<String>,
        cursor: Option<Box<dyn EdgeCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Single Input ============
    /// Filter executor with expression-based predicates
    Filter {
        input: Box<StreamingExecutor>,
        predicate: Expression,
        opened: bool,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Project executor with expression-based column selection
    Project {
        input: Box<StreamingExecutor>,
        output_expressions: Vec<Expression>,
        output_col_names: Vec<String>,
        opened: bool,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Limit executor
    Limit {
        input: Box<StreamingExecutor>,
        limit: u32,
        consumed: u32,
        opened: bool,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Stateful ============
    /// Aggregate executor with GROUP BY and aggregate functions
    Aggregate {
        input: Box<StreamingExecutor>,
        /// GROUP BY expressions to compute group keys
        group_by_expressions: Vec<Expression>,
        /// (AggregateFunction, field_expression) pairs for aggregation
        aggregate_functions: Vec<(AggregateFunction, Expression)>,
        /// Buffer for collecting all input rows before aggregation
        all_rows: Vec<Vec<Value>>,
        /// Iterator for result chunks
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Sort executor with ORDER BY support
    Sort {
        input: Box<StreamingExecutor>,
        /// ORDER BY expressions
        sort_expressions: Vec<Expression>,
        /// Sort direction for each expression
        sort_directions: Vec<SortDirection>,
        all_rows: Vec<Vec<Value>>,
        row_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Binary Input ============
    /// HashJoin executor with join condition support
    /// Builds a HashMap on the right side for O(1) probe lookup.
    /// For condition-less joins (Cartesian product), all_right_rows stores all right rows.
    HashJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Join condition expression (None means Cartesian product)
        join_condition: Option<Expression>,
        /// Expressions evaluated on right rows to build the hash key.
        hash_keys: Vec<Expression>,
        /// Expressions evaluated on left rows to probe the hash table.
        probe_keys: Vec<Expression>,
        /// Hash table: join key values -> matching right rows
        build_side_hash: std::collections::HashMap<Vec<Value>, Vec<Vec<Value>>>,
        /// All right rows (for Cartesian product when no join_condition)
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        opened: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Column names from the right input (captured at build time).
        right_col_names: Vec<String>,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// GroupBy executor for independent grouping before aggregation
    GroupBy {
        input: Box<StreamingExecutor>,
        /// GROUP BY expressions to compute group keys
        group_by_expressions: Vec<Expression>,
        /// Buffer for collecting all input rows before grouping
        all_rows: Vec<Vec<Value>>,
        /// Iterator for result chunks
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Distinct executor to eliminate duplicate rows
    Distinct {
        input: Box<StreamingExecutor>,
        /// Set of already-seen rows (as serialized strings)
        seen_rows: std::collections::HashSet<String>,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// NestedLoopJoin for theta-joins and non-equi joins
    NestedLoopJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Join condition expression (can be any comparison)
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        opened: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// WindowFunction executor for analytic functions
    /// Buffers input by PARTITION BY clause, computes window functions
    WindowFunction {
        input: Box<StreamingExecutor>,
        /// Window function expressions
        window_exprs: Vec<Expression>,
        /// PARTITION BY expressions (empty means all rows in one partition)
        partition_by_exprs: Vec<Expression>,
        /// ORDER BY expressions
        order_by_exprs: Vec<Expression>,
        /// Sort directions for ORDER BY
        order_by_directions: Vec<SortDirection>,
        /// All input rows buffered
        all_rows: Vec<Vec<Value>>,
        /// Iterator for result chunks
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Set Union operation (combines all rows from left and right)
    Union {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Already-seen rows to eliminate duplicates
        seen_rows: std::collections::HashSet<String>,
        left_consumed: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Set UnionAll operation (combines all rows without deduplication)
    UnionAll {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        left_consumed: bool,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Set Intersect operation (returns rows present in both inputs)
    Intersect {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Rows from left side (stored as Vec for output generation)
        left_rows: Vec<Vec<Value>>,
        /// Rows from right side (stored as HashSet for O(1) lookup)
        right_rows: std::collections::HashSet<String>,
        left_buffered: bool,
        right_buffered: bool,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Set Except/Minus operation (returns rows from left not in right)
    Except {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Rows to exclude (from right side)
        exclude_rows: std::collections::HashSet<String>,
        right_buffered: bool,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Access Operations ============
    Start {
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    GetVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    GetEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        direction: String,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    EdgeIndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    IndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: Option<String>,
        index_value: Option<Value>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    Argument {
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    Sample {
        input: Box<StreamingExecutor>,
        count: u64,
        consumed: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Property & Index Lookup ============
    /// Get properties from vertices or edges by ID
    GetProp {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        edge_ids: Option<Vec<Value>>,
        prop_names: Vec<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Lookup vertices by index key
    LookupIndex {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: String,
        index_condition: Option<(String, Value)>,
        limit: Option<usize>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Join Operations (stub) ============
    InnerJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    LeftJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    RightJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        right_consumed: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    FullOuterJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        left_rows: Vec<Vec<Value>>,
        right_rows: Vec<Vec<Value>>,
        matched_right_indices: std::collections::HashSet<usize>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        phase: FullOuterJoinPhase,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    CrossJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        all_left_rows: Vec<Vec<Value>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        right_consumed: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    SemiJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        right_rows: Vec<Vec<Value>>,
        right_consumed: bool,
        /// Per-operator memory tracker.
        memory_tracker: MemoryTracker,
        /// Plan node ID for debugging and tracking
        plan_node_id: i64,
        opened: bool,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Graph Traversal Operations ============
    Expand {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: String,
        direction: String,
        filter_expr: Option<Expression>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    ExpandAll {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: String,
        direction: String,
        filter_expr: Option<Expression>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Traverse {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: String,
        direction: String,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        visited: std::collections::HashSet<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    TraverseAll {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: String,
        direction: String,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        visited: std::collections::HashSet<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    AppendVertices {
        input: Box<StreamingExecutor>,
        vertex_properties: Vec<(String, Expression)>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    BiExpand {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: String,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    BiTraverse {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: String,
        min_depth: u32,
        max_depth: u32,
        visited: std::collections::HashSet<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    ShortestPath {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_type: String,
        direction: String,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    BFSShortest {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_type: String,
        direction: String,
        frontier: Vec<Vec<Value>>,
        visited: std::collections::HashSet<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    AllPaths {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_type: String,
        direction: String,
        all_paths: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    MultiShortestPath {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertices: Vec<Expression>,
        edge_type: String,
        direction: String,
        all_paths: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    /// Subgraph extraction: find subgraph within N steps from seed vertices
    Subgraph {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        steps: u32,
        direction: String,
        edge_types: Vec<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Data Modification ============
    InsertVertices {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_properties: Vec<(String, Expression)>,
        tags: Vec<String>,
        rows_inserted: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    InsertEdges {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        edge_properties: Vec<(String, Expression)>,
        rows_inserted: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    UpdateVertices {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        updates: Vec<(String, Expression)>,
        rows_updated: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    UpdateEdges {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        updates: Vec<(String, Expression)>,
        rows_updated: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    DeleteVertices {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_id_col: String,
        rows_deleted: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    DeleteEdges {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        rows_deleted: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    PipeDeleteVertices {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_id_col: String,
        rows_deleted: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    PipeDeleteEdges {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        rows_deleted: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    DeleteTags {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        tag_names: Vec<String>,
        vertex_ids: Option<Vec<Value>>,
        rows_deleted: u64,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Search Operations ============
    FulltextSearch {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    FulltextLookup {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    MatchFulltext {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        match_expr: Expression,
        match_field: Option<String>,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    VectorSearch {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        query_vector: Vec<f32>,
        top_k: u32,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    VectorLookup {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: String,
        lookup_key: Expression,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    VectorMatch {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        pattern: String,
        field: String,
        query_vector: Vec<f32>,
        threshold: Option<f32>,
        tag_name: String,
        field_name: String,
        space_id: u64,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
    SpaceManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        action: String,
        space_name: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    TagManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        tag_name: Option<String>,
        properties: Vec<crate::core::types::PropertyDef>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    EdgeManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        edge_type: Option<String>,
        properties: Vec<crate::core::types::PropertyDef>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    IndexManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        index_name: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    UserManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        action: String,
        username: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    FulltextManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        action: String,
        index_name: Option<String>,
        tag_name: Option<String>,
        field_name: Option<String>,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    VectorManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        action: String,
        index_name: Option<String>,
        tag_name: Option<String>,
        field_name: Option<String>,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Simple Relational Operations ============
    TopN {
        input: Box<StreamingExecutor>,
        n: u32,
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        all_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Dedup {
        input: Box<StreamingExecutor>,
        seen_rows: std::collections::HashSet<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Assign {
        input: Box<StreamingExecutor>,
        assignments: Vec<(String, Expression)>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Materialize {
        input: Box<StreamingExecutor>,
        materialized_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        materialized: bool,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Remove {
        input: Box<StreamingExecutor>,
        columns_to_remove: Vec<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    DataCollect {
        input: Box<StreamingExecutor>,
        all_rows: Vec<Vec<Value>>,
        emitted: bool,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Unwind {
        input: Box<StreamingExecutor>,
        unwind_column: String,
        col_index: Option<usize>,
        all_rows: Vec<Vec<Value>>,
        current_row_index: usize,
        current_unwind_index: usize,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Apply {
        input: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        apply_expression: Expression,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    PatternApply {
        input: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        pattern: Expression,
        all_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    RollUpApply {
        input: Box<StreamingExecutor>,
        rollup_expressions: Vec<Expression>,
        all_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Minus {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        exclude_rows: std::collections::HashSet<String>,
        right_buffered: bool,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Window {
        input: Box<StreamingExecutor>,
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
        all_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
        memory_tracker: MemoryTracker,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Control Flow ============
    Loop {
        input: Box<StreamingExecutor>,
        condition: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Select {
        input: Box<StreamingExecutor>,
        selection_expr: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    PassThrough {
        input: Box<StreamingExecutor>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    BeginTransaction {
        input: Box<StreamingExecutor>,
        transaction_id: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Commit {
        input: Box<StreamingExecutor>,
        transaction_id: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Rollback {
        input: Box<StreamingExecutor>,
        transaction_id: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Other (stub) ============
    ShowStats {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    // ============ Analysis & Migration ============
    Analyze {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        analyze_target: String,
        target_name: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },

    Migrate {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        migration_data: Option<String>,
        opened: bool,
        plan_node_id: i64,
        runtime: Option<Arc<ExecutionRuntime>>,
    },
}

impl StreamingExecutor {
    /// Recursively set the runtime on this operator and all children.
    pub fn set_runtime(&mut self, rt: Option<Arc<ExecutionRuntime>>) {
        use StreamingExecutor::*;
        match self {
            // Leaf operators (no children)
            Start { ref mut runtime, .. }
            | GetVertices { ref mut runtime, .. }
            | GetEdges { ref mut runtime, .. }
            | GetNeighbors { ref mut runtime, .. }
            | EdgeIndexScan { ref mut runtime, .. }
            | IndexScan { ref mut runtime, .. }
            | Argument { ref mut runtime, .. }
            | GetProp { ref mut runtime, .. }
            | LookupIndex { ref mut runtime, .. }
            | ScanVertices { ref mut runtime, .. }
            | StorageScanVertices { ref mut runtime, .. }
            | ScanEdges { ref mut runtime, .. }
            | StorageScanEdges { ref mut runtime, .. } => *runtime = rt.clone(),

            // Single-input: set self then recurse
            Filter { ref mut runtime, input, .. } | Project { ref mut runtime, input, .. }
            | Limit { ref mut runtime, input, .. } | Distinct { ref mut runtime, input, .. }
            | Aggregate { ref mut runtime, input, .. } | Sort { ref mut runtime, input, .. }
            | GroupBy { ref mut runtime, input, .. }
            | WindowFunction { ref mut runtime, input, .. }
            | TopN { ref mut runtime, input, .. } | Dedup { ref mut runtime, input, .. }
            | Assign { ref mut runtime, input, .. }
            | Materialize { ref mut runtime, input, .. }
            | Remove { ref mut runtime, input, .. }
            | DataCollect { ref mut runtime, input, .. }
            | Unwind { ref mut runtime, input, .. }
            | RollUpApply { ref mut runtime, input, .. }
            | Window { ref mut runtime, input, .. }
            | Loop { ref mut runtime, input, .. }
            | Select { ref mut runtime, input, .. }
            | PassThrough { ref mut runtime, input, .. }
            | BeginTransaction { ref mut runtime, input, .. }
            | Commit { ref mut runtime, input, .. }
            | Rollback { ref mut runtime, input, .. }
            | ShowStats { ref mut runtime, input, .. }
            | Analyze { ref mut runtime, input, .. }
            | Migrate { ref mut runtime, input, .. }
            | Sample { ref mut runtime, input, .. }
            | Expand { ref mut runtime, input, .. }
            | ExpandAll { ref mut runtime, input, .. }
            | Traverse { ref mut runtime, input, .. }
            | TraverseAll { ref mut runtime, input, .. }
            | AppendVertices { ref mut runtime, input, .. }
            | BiExpand { ref mut runtime, input, .. }
            | BiTraverse { ref mut runtime, input, .. }
            | ShortestPath { ref mut runtime, input, .. }
            | BFSShortest { ref mut runtime, input, .. }
            | AllPaths { ref mut runtime, input, .. }
            | MultiShortestPath { ref mut runtime, input, .. }
            | Subgraph { ref mut runtime, input, .. }
            | InsertVertices { ref mut runtime, input, .. }
            | InsertEdges { ref mut runtime, input, .. }
            | UpdateVertices { ref mut runtime, input, .. }
            | UpdateEdges { ref mut runtime, input, .. }
            | DeleteVertices { ref mut runtime, input, .. }
            | DeleteEdges { ref mut runtime, input, .. }
            | PipeDeleteVertices { ref mut runtime, input, .. }
            | PipeDeleteEdges { ref mut runtime, input, .. }
            | DeleteTags { ref mut runtime, input, .. }
            | PatternApply { ref mut runtime, input, .. }
            | FulltextSearch { ref mut runtime, input, .. }
            | FulltextLookup { ref mut runtime, input, .. }
            | MatchFulltext { ref mut runtime, input, .. }
            | VectorSearch { ref mut runtime, input, .. }
            | VectorLookup { ref mut runtime, input, .. }
            | VectorMatch { ref mut runtime, input, .. }
            | SpaceManage { ref mut runtime, input, .. }
            | TagManage { ref mut runtime, input, .. }
            | EdgeManage { ref mut runtime, input, .. }
            | IndexManage { ref mut runtime, input, .. }
            | UserManage { ref mut runtime, input, .. }
            | FulltextManage { ref mut runtime, input, .. }
            | VectorManage { ref mut runtime, input, .. } => {
                *runtime = rt.clone();
                input.set_runtime(rt.clone());
            }

            // Binary-input operators
            HashJoin { ref mut runtime, left, right, .. }
            | NestedLoopJoin { ref mut runtime, left, right, .. }
            | InnerJoin { ref mut runtime, left, right, .. }
            | LeftJoin { ref mut runtime, left, right, .. }
            | RightJoin { ref mut runtime, left, right, .. }
            | FullOuterJoin { ref mut runtime, left, right, .. }
            | CrossJoin { ref mut runtime, left, right, .. }
            | SemiJoin { ref mut runtime, left, right, .. }
            | Union { ref mut runtime, left, right, .. }
            | UnionAll { ref mut runtime, left, right, .. }
            | Intersect { ref mut runtime, left, right, .. }
            | Except { ref mut runtime, left, right, .. }
            | Minus { ref mut runtime, left, right, .. }
            | Apply { ref mut runtime, input: left, right, .. } => {
                *runtime = rt.clone();
                left.set_runtime(rt.clone());
                right.set_runtime(rt.clone());
            }
        }
    }

    /// Return the plan node ID of this operator.
    pub fn plan_node_id(&self) -> i64 {
        use StreamingExecutor::*;
        match self {
            ScanVertices { plan_node_id, .. }
            | StorageScanVertices { plan_node_id, .. }
            | ScanEdges { plan_node_id, .. }
            | StorageScanEdges { plan_node_id, .. }
            | Filter { plan_node_id, .. }
            | Project { plan_node_id, .. }
            | Limit { plan_node_id, .. }
            | Sort { plan_node_id, .. }
            | Aggregate { plan_node_id, .. }
            | HashJoin { plan_node_id, .. }
            | InnerJoin { plan_node_id, .. }
            | LeftJoin { plan_node_id, .. }
            | RightJoin { plan_node_id, .. }
            | FullOuterJoin { plan_node_id, .. }
            | CrossJoin { plan_node_id, .. }
            | SemiJoin { plan_node_id, .. }
            | NestedLoopJoin { plan_node_id, .. }
            | GroupBy { plan_node_id, .. }
            | Distinct { plan_node_id, .. }
            | WindowFunction { plan_node_id, .. }
            | Union { plan_node_id, .. }
            | UnionAll { plan_node_id, .. }
            | Intersect { plan_node_id, .. }
            | Except { plan_node_id, .. }
            | Minus { plan_node_id, .. }
            | Start { plan_node_id, .. }
            | GetVertices { plan_node_id, .. }
            | GetEdges { plan_node_id, .. }
            | GetNeighbors { plan_node_id, .. }
            | EdgeIndexScan { plan_node_id, .. }
            | IndexScan { plan_node_id, .. }
            | Argument { plan_node_id, .. }
            | Sample { plan_node_id, .. }
            | GetProp { plan_node_id, .. }
            | LookupIndex { plan_node_id, .. }
            | Expand { plan_node_id, .. }
            | ExpandAll { plan_node_id, .. }
            | Traverse { plan_node_id, .. }
            | TraverseAll { plan_node_id, .. }
            | AppendVertices { plan_node_id, .. }
            | BiExpand { plan_node_id, .. }
            | BiTraverse { plan_node_id, .. }
            | ShortestPath { plan_node_id, .. }
            | BFSShortest { plan_node_id, .. }
            | AllPaths { plan_node_id, .. }
            | MultiShortestPath { plan_node_id, .. }
            | Subgraph { plan_node_id, .. }
            | InsertVertices { plan_node_id, .. }
            | InsertEdges { plan_node_id, .. }
            | UpdateVertices { plan_node_id, .. }
            | UpdateEdges { plan_node_id, .. }
            | DeleteVertices { plan_node_id, .. }
            | DeleteEdges { plan_node_id, .. }
            | PipeDeleteVertices { plan_node_id, .. }
            | PipeDeleteEdges { plan_node_id, .. }
            | DeleteTags { plan_node_id, .. }
            | TopN { plan_node_id, .. }
            | Dedup { plan_node_id, .. }
            | Assign { plan_node_id, .. }
            | Materialize { plan_node_id, .. }
            | Remove { plan_node_id, .. }
            | DataCollect { plan_node_id, .. }
            | Unwind { plan_node_id, .. }
            | Apply { plan_node_id, .. }
            | PatternApply { plan_node_id, .. }
            | RollUpApply { plan_node_id, .. }
            | Window { plan_node_id, .. }
            | Loop { plan_node_id, .. }
            | Select { plan_node_id, .. }
            | PassThrough { plan_node_id, .. }
            | BeginTransaction { plan_node_id, .. }
            | Commit { plan_node_id, .. }
            | Rollback { plan_node_id, .. }
            | ShowStats { plan_node_id, .. }
            | Analyze { plan_node_id, .. }
            | Migrate { plan_node_id, .. }
            | SpaceManage { plan_node_id, .. }
            | TagManage { plan_node_id, .. }
            | EdgeManage { plan_node_id, .. }
            | IndexManage { plan_node_id, .. }
            | UserManage { plan_node_id, .. }
            | FulltextManage { plan_node_id, .. }
            | VectorManage { plan_node_id, .. }
            | FulltextSearch { plan_node_id, .. }
            | FulltextLookup { plan_node_id, .. }
            | MatchFulltext { plan_node_id, .. }
            | VectorSearch { plan_node_id, .. }
            | VectorLookup { plan_node_id, .. }
            | VectorMatch { plan_node_id, .. } => *plan_node_id,
        }
    }

    /// Access the runtime reference, if attached.
    pub fn get_runtime(&self) -> Option<&ExecutionRuntime> {
        use StreamingExecutor::*;
        let rt = match self {
            ScanVertices { runtime, .. }
            | StorageScanVertices { runtime, .. }
            | ScanEdges { runtime, .. }
            | StorageScanEdges { runtime, .. }
            | Filter { runtime, .. }
            | Project { runtime, .. }
            | Limit { runtime, .. }
            | Sort { runtime, .. }
            | Aggregate { runtime, .. }
            | HashJoin { runtime, .. }
            | InnerJoin { runtime, .. }
            | LeftJoin { runtime, .. }
            | RightJoin { runtime, .. }
            | FullOuterJoin { runtime, .. }
            | CrossJoin { runtime, .. }
            | SemiJoin { runtime, .. }
            | NestedLoopJoin { runtime, .. }
            | GroupBy { runtime, .. }
            | Distinct { runtime, .. }
            | WindowFunction { runtime, .. }
            | Union { runtime, .. }
            | UnionAll { runtime, .. }
            | Intersect { runtime, .. }
            | Except { runtime, .. }
            | Minus { runtime, .. }
            | Start { runtime, .. }
            | GetVertices { runtime, .. }
            | GetEdges { runtime, .. }
            | GetNeighbors { runtime, .. }
            | EdgeIndexScan { runtime, .. }
            | IndexScan { runtime, .. }
            | Argument { runtime, .. }
            | Sample { runtime, .. }
            | GetProp { runtime, .. }
            | LookupIndex { runtime, .. }
            | Expand { runtime, .. }
            | ExpandAll { runtime, .. }
            | Traverse { runtime, .. }
            | TraverseAll { runtime, .. }
            | AppendVertices { runtime, .. }
            | BiExpand { runtime, .. }
            | BiTraverse { runtime, .. }
            | ShortestPath { runtime, .. }
            | BFSShortest { runtime, .. }
            | AllPaths { runtime, .. }
            | MultiShortestPath { runtime, .. }
            | Subgraph { runtime, .. }
            | InsertVertices { runtime, .. }
            | InsertEdges { runtime, .. }
            | UpdateVertices { runtime, .. }
            | UpdateEdges { runtime, .. }
            | DeleteVertices { runtime, .. }
            | DeleteEdges { runtime, .. }
            | PipeDeleteVertices { runtime, .. }
            | PipeDeleteEdges { runtime, .. }
            | DeleteTags { runtime, .. }
            | TopN { runtime, .. }
            | Dedup { runtime, .. }
            | Assign { runtime, .. }
            | Materialize { runtime, .. }
            | Remove { runtime, .. }
            | DataCollect { runtime, .. }
            | Unwind { runtime, .. }
            | Apply { runtime, .. }
            | PatternApply { runtime, .. }
            | RollUpApply { runtime, .. }
            | Window { runtime, .. }
            | Loop { runtime, .. }
            | Select { runtime, .. }
            | PassThrough { runtime, .. }
            | BeginTransaction { runtime, .. }
            | Commit { runtime, .. }
            | Rollback { runtime, .. }
            | ShowStats { runtime, .. }
            | Analyze { runtime, .. }
            | Migrate { runtime, .. }
            | SpaceManage { runtime, .. }
            | TagManage { runtime, .. }
            | EdgeManage { runtime, .. }
            | IndexManage { runtime, .. }
            | UserManage { runtime, .. }
            | FulltextManage { runtime, .. }
            | VectorManage { runtime, .. }
            | FulltextSearch { runtime, .. }
            | FulltextLookup { runtime, .. }
            | MatchFulltext { runtime, .. }
            | VectorSearch { runtime, .. }
            | VectorLookup { runtime, .. }
            | VectorMatch { runtime, .. } => runtime,
        };
        rt.as_ref().map(|r| r.as_ref())
    }

    /// Check cancellation via the attached runtime.
    pub fn ensure_not_cancelled(&self) -> Result<(), QueryError> {
        if let Some(rt) = self.get_runtime() {
            rt.ensure_not_cancelled()
        } else {
            Ok(())
        }
    }

    /// Record profile timing for this operator.
    pub fn record_profile_timing(
        &self,
        phase: &str,
        elapsed_us: u64,
    ) {
        if let Some(rt) = self.get_runtime() {
            let node_id = self.plan_node_id();
            let name = driver::extract_operator_name(self);
            let mut profile = rt.profile().lock();
            let entry = profile.operators.entry(node_id).or_insert_with(|| {
                use super::runtime::OperatorProfile;
                OperatorProfile { node_id, name, ..OperatorProfile::default() }
            });
            match phase {
                "open" => entry.open_time_us += elapsed_us,
                "next" => entry.next_time_us += elapsed_us,
                "close" => entry.close_time_us += elapsed_us,
                _ => {}
            }
        }
    }

    /// Get peak memory from the memory_tracker, if this operator has one.
    pub fn peak_memory_bytes(&self) -> u64 {
        let extract = |mt: &MemoryTracker| mt.peak() as u64;
        match self {
            Self::Distinct { memory_tracker, .. }
            | Self::Aggregate { memory_tracker, .. }
            | Self::Sort { memory_tracker, .. }
            | Self::GroupBy { memory_tracker, .. }
            | Self::WindowFunction { memory_tracker, .. }
            | Self::HashJoin { memory_tracker, .. }
            | Self::NestedLoopJoin { memory_tracker, .. }
            | Self::InnerJoin { memory_tracker, .. }
            | Self::LeftJoin { memory_tracker, .. }
            | Self::RightJoin { memory_tracker, .. }
            | Self::FullOuterJoin { memory_tracker, .. }
            | Self::CrossJoin { memory_tracker, .. }
            | Self::SemiJoin { memory_tracker, .. }
            | Self::Union { memory_tracker, .. }
            | Self::Intersect { memory_tracker, .. }
            | Self::Except { memory_tracker, .. }
            | Self::Minus { memory_tracker, .. }
            | Self::TopN { memory_tracker, .. }
            | Self::Materialize { memory_tracker, .. }
            | Self::DataCollect { memory_tracker, .. }
            | Self::Window { memory_tracker, .. }
            | Self::RollUpApply { memory_tracker, .. }
            | Self::PatternApply { memory_tracker, .. } => extract(memory_tracker),
            _ => 0,
        }
    }

    /// Record output row count in profile for this operator.
    pub fn record_profile_rows(&self, count: u64) {
        if let Some(rt) = self.get_runtime() {
            let node_id = self.plan_node_id();
            let mut profile = rt.profile().lock();
            if let Some(entry) = profile.operators.get_mut(&node_id) {
                entry.output_rows += count;
            }
            profile.add_rows(count);
        }
    }

    /// Record peak memory usage in profile for this operator.
    pub fn record_profile_peak_memory(&self, bytes: u64) {
        if let Some(rt) = self.get_runtime() {
            let node_id = self.plan_node_id();
            let mut profile = rt.profile().lock();
            let entry = profile.operators.get_mut(&node_id);
            if let Some(entry) = entry {
                if bytes > entry.peak_memory_bytes {
                    entry.peak_memory_bytes = bytes;
                }
            }
        }
    }

    /// Register a resource cleanup callback with the attached runtime.
    pub fn register_resource<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if let Some(rt) = self.get_runtime() {
            rt.on_cleanup(f);
        }
    }

    /// Initialize the executor
    pub fn open(&mut self) -> Result<(), QueryError> {
        self.ensure_not_cancelled()?;
        let start = Instant::now();
        let result = match self {
            // Access operations
            Self::Start { .. } => operators::access::open_start(self),
            Self::GetVertices { .. } => operators::access::open_getvertices(self),
            Self::GetEdges { .. } => operators::access::open_getedges(self),
            Self::GetNeighbors { .. } => operators::access::open_getneighbors(self),
            Self::IndexScan { .. } => operators::access::open_indexscan(self),
            Self::EdgeIndexScan { .. } => operators::access::open_edgeindexscan(self),
            Self::Argument { .. } => operators::access::open_argument(self),
            Self::Sample { .. } => operators::access::open_sample(self),
            // Property & Index lookup
            Self::GetProp { .. } => operators::access::open_getprop(self),
            Self::LookupIndex { .. } => operators::access::open_lookupindex(self),
            // Data source operations
            Self::ScanVertices { .. } | Self::StorageScanVertices { .. } => {
                operators::sources::open_scanvertices(self)
            }
            Self::ScanEdges { .. } | Self::StorageScanEdges { .. } => {
                operators::sources::open_scanedges(self)
            }
            // Single input operations
            Self::Filter { .. } => operators::single_input::open_filter(self),
            Self::Project { .. } => operators::single_input::open_project(self),
            Self::Limit { .. } => operators::single_input::open_limit(self),
            Self::Distinct { .. } => operators::single_input::open_distinct(self),
            // Stateful operations
            Self::Aggregate { .. } => operators::stateful::open_aggregate(self),
            Self::Sort { .. } => operators::stateful::open_sort(self),
            Self::GroupBy { .. } => operators::stateful::open_groupby(self),
            Self::WindowFunction { .. } => operators::stateful::open_windowfunction(self),
            // Binary operations
            Self::HashJoin { .. } => operators::binary::open_hashjoin(self),
            Self::NestedLoopJoin { .. } => operators::binary::open_nestedloopjoin(self),
            Self::InnerJoin { .. } => operators::binary::open_innerjoin(self),
            Self::LeftJoin { .. } => operators::binary::open_leftjoin(self),
            Self::RightJoin { .. } => operators::binary::open_rightjoin(self),
            Self::FullOuterJoin { .. } => operators::binary::open_fullouterjoin(self),
            Self::CrossJoin { .. } => operators::binary::open_crossjoin(self),
            Self::SemiJoin { .. } => operators::binary::open_semijoin(self),
            // Set operations
            Self::Union { .. } => operators::set_ops::open_union(self),
            Self::UnionAll { .. } => operators::set_ops::open_unionall(self),
            Self::Intersect { .. } => operators::set_ops::open_intersect(self),
            Self::Except { .. } => operators::set_ops::open_except(self),
            // Relational operations
            Self::TopN { .. } => operators::relational::open_topn(self),
            Self::Dedup { .. } => operators::relational::open_dedup(self),
            Self::Assign { .. } => operators::relational::open_assign(self),
            Self::Materialize { .. } => operators::relational::open_materialize(self),
            Self::Remove { .. } => operators::relational::open_remove(self),
            Self::DataCollect { .. } => operators::relational::open_datacollect(self),
            Self::Unwind { .. } => operators::relational::open_unwind(self),
            Self::Apply { .. } => operators::relational::open_apply(self),
            Self::PatternApply { .. } => operators::relational::open_patternapply(self),
            Self::RollUpApply { .. } => operators::relational::open_rolluapply(self),
            Self::Minus { .. } => operators::relational::open_minus(self),
            Self::Window { .. } => operators::relational::open_window(self),
            // Data modification operations
            Self::InsertVertices { .. } => operators::data_modification::open_insertvertices(self),
            Self::InsertEdges { .. } => operators::data_modification::open_insertedges(self),
            Self::UpdateVertices { .. } => operators::data_modification::open_updatevertices(self),
            Self::UpdateEdges { .. } => operators::data_modification::open_updateedges(self),
            Self::DeleteVertices { .. } => operators::data_modification::open_deletevertices(self),
            Self::DeleteEdges { .. } => operators::data_modification::open_deleteedges(self),
            Self::PipeDeleteVertices { .. } => {
                operators::data_modification::open_pipedeletevertices(self)
            }
            Self::PipeDeleteEdges { .. } => {
                operators::data_modification::open_pipedeleteedges(self)
            }
            Self::DeleteTags { .. } => operators::data_modification::open_deletetags(self),
            // Graph traversal operations
            Self::Expand { .. } => operators::graph_traversal::open_expand(self),
            Self::ExpandAll { .. } => operators::graph_traversal::open_expandall(self),
            Self::Traverse { .. } => operators::graph_traversal::open_traverse(self),
            Self::TraverseAll { .. } => operators::graph_traversal::open_traverseall(self),
            Self::AppendVertices { .. } => operators::graph_traversal::open_appendvertices(self),
            Self::BiExpand { .. } => operators::graph_traversal::open_biexpand(self),
            Self::BiTraverse { .. } => operators::graph_traversal::open_bitraverse(self),
            Self::ShortestPath { .. } => operators::graph_traversal::open_shortestpath(self),
            Self::BFSShortest { .. } => operators::graph_traversal::open_bfsshortest(self),
            Self::AllPaths { .. } => operators::graph_traversal::open_allpaths(self),
            Self::MultiShortestPath { .. } => {
                operators::graph_traversal::open_multishortestpath(self)
            }
            Self::Subgraph { .. } => operators::graph_traversal::open_subgraph(self),
            // Search operations
            Self::FulltextSearch { .. } => operators::search::open_fulltext_search(self),
            Self::FulltextLookup { .. } => operators::search::open_fulltext_lookup(self),
            Self::MatchFulltext { .. } => operators::search::open_match_fulltext(self),
            Self::VectorSearch { .. } => operators::search::open_vector_search(self),
            Self::VectorLookup { .. } => operators::search::open_vector_lookup(self),
            Self::VectorMatch { .. } => operators::search::open_vector_match(self),
            // Management operations
            Self::SpaceManage { .. } => operators::management::open_space_manage(self),
            Self::TagManage { .. } => operators::management::open_tag_manage(self),
            Self::EdgeManage { .. } => operators::management::open_edge_manage(self),
            Self::IndexManage { .. } => operators::management::open_index_manage(self),
            Self::UserManage { .. } => operators::management::open_user_manage(self),
            Self::FulltextManage { .. } => operators::management::open_fulltext_manage(self),
            Self::VectorManage { .. } => operators::management::open_vector_manage(self),
            // Control flow operations
            Self::Loop { .. } => operators::control_flow::open_loop(self),
            Self::Select { .. } => operators::control_flow::open_select(self),
            Self::PassThrough { .. } => operators::control_flow::open_passthrough(self),
            Self::BeginTransaction { .. } => operators::control_flow::open_begin_transaction(self),
            Self::Commit { .. } => operators::control_flow::open_commit(self),
            Self::Rollback { .. } => operators::control_flow::open_rollback(self),
            Self::ShowStats { .. } => operators::control_flow::open_show_stats(self),
            // Analysis & Migration
            Self::Analyze { .. } => operators::management::open_analyze(self),
            Self::Migrate { .. } => operators::management::open_migrate(self),
        };
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("open", elapsed);
        result
    }

    /// Pull next chunk from the executor
    pub fn advance(&mut self) -> Result<Option<DataChunk>, QueryError> {
        self.ensure_not_cancelled()?;
        let start = Instant::now();
        let result = match self {
            // Access operations
            Self::Start { .. } => operators::access::next_start(self),
            Self::GetVertices { .. } => operators::access::next_getvertices(self),
            Self::GetEdges { .. } => operators::access::next_getedges(self),
            Self::GetNeighbors { .. } => operators::access::next_getneighbors(self),
            Self::IndexScan { .. } => operators::access::next_indexscan(self),
            Self::EdgeIndexScan { .. } => operators::access::next_edgeindexscan(self),
            Self::Argument { .. } => operators::access::next_argument(self),
            Self::Sample { .. } => operators::access::next_sample(self),
            // Property & Index lookup
            Self::GetProp { .. } => operators::access::next_getprop(self),
            Self::LookupIndex { .. } => operators::access::next_lookupindex(self),
            // Data source operations
            Self::ScanVertices { .. } | Self::StorageScanVertices { .. } => {
                operators::sources::next_scanvertices(self)
            }
            Self::ScanEdges { .. } | Self::StorageScanEdges { .. } => {
                operators::sources::next_scanedges(self)
            }
            // Single input operations
            Self::Filter { .. } => operators::single_input::next_filter(self),
            Self::Project { .. } => operators::single_input::next_project(self),
            Self::Limit { .. } => operators::single_input::next_limit(self),
            Self::Distinct { .. } => operators::single_input::next_distinct(self),
            // Stateful operations
            Self::Aggregate { .. } => operators::stateful::next_aggregate(self),
            Self::Sort { .. } => operators::stateful::next_sort(self),
            Self::GroupBy { .. } => operators::stateful::next_groupby(self),
            Self::WindowFunction { .. } => operators::stateful::next_windowfunction(self),
            // Binary operations
            Self::HashJoin { .. } => operators::binary::next_hashjoin(self),
            Self::NestedLoopJoin { .. } => operators::binary::next_nestedloopjoin(self),
            Self::InnerJoin { .. } => operators::binary::next_innerjoin(self),
            Self::LeftJoin { .. } => operators::binary::next_leftjoin(self),
            Self::RightJoin { .. } => operators::binary::next_rightjoin(self),
            Self::FullOuterJoin { .. } => operators::binary::next_fullouterjoin(self),
            Self::CrossJoin { .. } => operators::binary::next_crossjoin(self),
            Self::SemiJoin { .. } => operators::binary::next_semijoin(self),
            // Set operations
            Self::Union { .. } => operators::set_ops::next_union(self),
            Self::UnionAll { .. } => operators::set_ops::next_unionall(self),
            Self::Intersect { .. } => operators::set_ops::next_intersect(self),
            Self::Except { .. } => operators::set_ops::next_except(self),
            // Relational operations
            Self::TopN { .. } => operators::relational::next_topn(self),
            Self::Dedup { .. } => operators::relational::next_dedup(self),
            Self::Assign { .. } => operators::relational::next_assign(self),
            Self::Materialize { .. } => operators::relational::next_materialize(self),
            Self::Remove { .. } => operators::relational::next_remove(self),
            Self::DataCollect { .. } => operators::relational::next_datacollect(self),
            Self::Unwind { .. } => operators::relational::next_unwind(self),
            Self::Apply { .. } => operators::relational::next_apply(self),
            Self::PatternApply { .. } => operators::relational::next_patternapply(self),
            Self::RollUpApply { .. } => operators::relational::next_rolluapply(self),
            Self::Minus { .. } => operators::relational::next_minus(self),
            Self::Window { .. } => operators::relational::next_window(self),
            // Data modification operations
            Self::InsertVertices { .. } => operators::data_modification::next_insertvertices(self),
            Self::InsertEdges { .. } => operators::data_modification::next_insertedges(self),
            Self::UpdateVertices { .. } => operators::data_modification::next_updatevertices(self),
            Self::UpdateEdges { .. } => operators::data_modification::next_updateedges(self),
            Self::DeleteVertices { .. } => operators::data_modification::next_deletevertices(self),
            Self::DeleteEdges { .. } => operators::data_modification::next_deleteedges(self),
            Self::PipeDeleteVertices { .. } => {
                operators::data_modification::next_pipedeletevertices(self)
            }
            Self::PipeDeleteEdges { .. } => {
                operators::data_modification::next_pipedeleteedges(self)
            }
            Self::DeleteTags { .. } => operators::data_modification::next_deletetags(self),
            // Graph traversal operations
            Self::Expand { .. } => operators::graph_traversal::next_expand(self),
            Self::ExpandAll { .. } => operators::graph_traversal::next_expandall(self),
            Self::Traverse { .. } => operators::graph_traversal::next_traverse(self),
            Self::TraverseAll { .. } => operators::graph_traversal::next_traverseall(self),
            Self::AppendVertices { .. } => operators::graph_traversal::next_appendvertices(self),
            Self::BiExpand { .. } => operators::graph_traversal::next_biexpand(self),
            Self::BiTraverse { .. } => operators::graph_traversal::next_bitraverse(self),
            Self::ShortestPath { .. } => operators::graph_traversal::next_shortestpath(self),
            Self::BFSShortest { .. } => operators::graph_traversal::next_bfsshortest(self),
            Self::AllPaths { .. } => operators::graph_traversal::next_allpaths(self),
            Self::MultiShortestPath { .. } => {
                operators::graph_traversal::next_multishortestpath(self)
            }
            Self::Subgraph { .. } => operators::graph_traversal::next_subgraph(self),
            // Search operations
            Self::FulltextSearch { .. } => operators::search::next_fulltext_search(self),
            Self::FulltextLookup { .. } => operators::search::next_fulltext_lookup(self),
            Self::MatchFulltext { .. } => operators::search::next_match_fulltext(self),
            Self::VectorSearch { .. } => operators::search::next_vector_search(self),
            Self::VectorLookup { .. } => operators::search::next_vector_lookup(self),
            Self::VectorMatch { .. } => operators::search::next_vector_match(self),
            // Management operations
            Self::SpaceManage { .. } => operators::management::next_space_manage(self),
            Self::TagManage { .. } => operators::management::next_tag_manage(self),
            Self::EdgeManage { .. } => operators::management::next_edge_manage(self),
            Self::IndexManage { .. } => operators::management::next_index_manage(self),
            Self::UserManage { .. } => operators::management::next_user_manage(self),
            Self::FulltextManage { .. } => operators::management::next_fulltext_manage(self),
            Self::VectorManage { .. } => operators::management::next_vector_manage(self),
            // Control flow operations
            Self::Loop { .. } => operators::control_flow::next_loop(self),
            Self::Select { .. } => operators::control_flow::next_select(self),
            Self::PassThrough { .. } => operators::control_flow::next_passthrough(self),
            Self::BeginTransaction { .. } => operators::control_flow::next_begin_transaction(self),
            Self::Commit { .. } => operators::control_flow::next_commit(self),
            Self::Rollback { .. } => operators::control_flow::next_rollback(self),
            Self::ShowStats { .. } => operators::control_flow::next_show_stats(self),
            // Analysis & Migration
            Self::Analyze { .. } => operators::management::next_analyze(self),
            Self::Migrate { .. } => operators::management::next_migrate(self),
        };
        let elapsed = start.elapsed().as_micros() as u64;
        if let Ok(Some(ref chunk)) = result {
            self.record_profile_rows(chunk.len() as u64);
        }
        self.record_profile_timing("next", elapsed);
        result
    }

    /// Stop the executor (signal no more input needed)
    pub fn stop(&mut self) -> Result<(), QueryError> {
        self.ensure_not_cancelled()?;
        let start = Instant::now();
        let result = match self {
            // Access operations
            Self::Start { .. } => operators::access::stop_start(self),
            Self::GetVertices { .. } => operators::access::stop_getvertices(self),
            Self::GetEdges { .. } => operators::access::stop_getedges(self),
            Self::GetNeighbors { .. } => operators::access::stop_getneighbors(self),
            Self::IndexScan { .. } => operators::access::stop_indexscan(self),
            Self::EdgeIndexScan { .. } => operators::access::stop_edgeindexscan(self),
            Self::Argument { .. } => operators::access::stop_argument(self),
            Self::Sample { .. } => operators::access::stop_sample(self),
            // Property & Index lookup
            Self::GetProp { .. } => operators::access::stop_getprop(self),
            Self::LookupIndex { .. } => operators::access::stop_lookupindex(self),
            // Data source operations
            Self::ScanVertices { .. } | Self::StorageScanVertices { .. } => {
                operators::sources::stop_scanvertices(self)
            }
            Self::ScanEdges { .. } | Self::StorageScanEdges { .. } => {
                operators::sources::stop_scanedges(self)
            }
            // Single input operations
            Self::Filter { .. } => operators::single_input::stop_filter(self),
            Self::Project { .. } => operators::single_input::stop_project(self),
            Self::Limit { .. } => operators::single_input::stop_limit(self),
            Self::Distinct { .. } => operators::single_input::stop_distinct(self),
            // Stateful operations
            Self::Aggregate { .. } => operators::stateful::stop_aggregate(self),
            Self::Sort { .. } => operators::stateful::stop_sort(self),
            Self::GroupBy { .. } => operators::stateful::stop_groupby(self),
            Self::WindowFunction { .. } => operators::stateful::stop_windowfunction(self),
            // Binary operations
            Self::HashJoin { .. } => operators::binary::stop_hashjoin(self),
            Self::NestedLoopJoin { .. } => operators::binary::stop_nestedloopjoin(self),
            Self::InnerJoin { .. } => operators::binary::stop_innerjoin(self),
            Self::LeftJoin { .. } => operators::binary::stop_leftjoin(self),
            Self::RightJoin { .. } => operators::binary::stop_rightjoin(self),
            Self::FullOuterJoin { .. } => operators::binary::stop_fullouterjoin(self),
            Self::CrossJoin { .. } => operators::binary::stop_crossjoin(self),
            Self::SemiJoin { .. } => operators::binary::stop_semijoin(self),
            // Set operations
            Self::Union { .. } => operators::set_ops::stop_union(self),
            Self::UnionAll { .. } => operators::set_ops::stop_unionall(self),
            Self::Intersect { .. } => operators::set_ops::stop_intersect(self),
            Self::Except { .. } => operators::set_ops::stop_except(self),
            // Relational operations
            Self::TopN { .. } => operators::relational::stop_topn(self),
            Self::Dedup { .. } => operators::relational::stop_dedup(self),
            Self::Assign { .. } => operators::relational::stop_assign(self),
            Self::Materialize { .. } => operators::relational::stop_materialize(self),
            Self::Remove { .. } => operators::relational::stop_remove(self),
            Self::DataCollect { .. } => operators::relational::stop_datacollect(self),
            Self::Unwind { .. } => operators::relational::stop_unwind(self),
            Self::Apply { .. } => operators::relational::stop_apply(self),
            Self::PatternApply { .. } => operators::relational::stop_patternapply(self),
            Self::RollUpApply { .. } => operators::relational::stop_rolluapply(self),
            Self::Minus { .. } => operators::relational::stop_minus(self),
            Self::Window { .. } => operators::relational::stop_window(self),
            // Data modification operations
            Self::InsertVertices { .. } => operators::data_modification::stop_insertvertices(self),
            Self::InsertEdges { .. } => operators::data_modification::stop_insertedges(self),
            Self::UpdateVertices { .. } => operators::data_modification::stop_updatevertices(self),
            Self::UpdateEdges { .. } => operators::data_modification::stop_updateedges(self),
            Self::DeleteVertices { .. } => operators::data_modification::stop_deletevertices(self),
            Self::DeleteEdges { .. } => operators::data_modification::stop_deleteedges(self),
            Self::PipeDeleteVertices { .. } => {
                operators::data_modification::stop_pipedeletevertices(self)
            }
            Self::PipeDeleteEdges { .. } => {
                operators::data_modification::stop_pipedeleteedges(self)
            }
            Self::DeleteTags { .. } => operators::data_modification::stop_deletetags(self),
            // Graph traversal operations
            Self::Expand { .. } => operators::graph_traversal::stop_expand(self),
            Self::ExpandAll { .. } => operators::graph_traversal::stop_expandall(self),
            Self::Traverse { .. } => operators::graph_traversal::stop_traverse(self),
            Self::TraverseAll { .. } => operators::graph_traversal::stop_traverseall(self),
            Self::AppendVertices { .. } => operators::graph_traversal::stop_appendvertices(self),
            Self::BiExpand { .. } => operators::graph_traversal::stop_biexpand(self),
            Self::BiTraverse { .. } => operators::graph_traversal::stop_bitraverse(self),
            Self::ShortestPath { .. } => operators::graph_traversal::stop_shortestpath(self),
            Self::BFSShortest { .. } => operators::graph_traversal::stop_bfsshortest(self),
            Self::AllPaths { .. } => operators::graph_traversal::stop_allpaths(self),
            Self::MultiShortestPath { .. } => {
                operators::graph_traversal::stop_multishortestpath(self)
            }
            Self::Subgraph { .. } => operators::graph_traversal::stop_subgraph(self),
            // Search operations
            Self::FulltextSearch { .. } => operators::search::stop_fulltext_search(self),
            Self::FulltextLookup { .. } => operators::search::stop_fulltext_lookup(self),
            Self::MatchFulltext { .. } => operators::search::stop_match_fulltext(self),
            Self::VectorSearch { .. } => operators::search::stop_vector_search(self),
            Self::VectorLookup { .. } => operators::search::stop_vector_lookup(self),
            Self::VectorMatch { .. } => operators::search::stop_vector_match(self),
            // Management operations
            Self::SpaceManage { .. } => operators::management::stop_space_manage(self),
            Self::TagManage { .. } => operators::management::stop_tag_manage(self),
            Self::EdgeManage { .. } => operators::management::stop_edge_manage(self),
            Self::IndexManage { .. } => operators::management::stop_index_manage(self),
            Self::UserManage { .. } => operators::management::stop_user_manage(self),
            Self::FulltextManage { .. } => operators::management::stop_fulltext_manage(self),
            Self::VectorManage { .. } => operators::management::stop_vector_manage(self),
            // Control flow operations
            Self::Loop { .. } => operators::control_flow::stop_loop(self),
            Self::Select { .. } => operators::control_flow::stop_select(self),
            Self::PassThrough { .. } => operators::control_flow::stop_passthrough(self),
            Self::BeginTransaction { .. } => operators::control_flow::stop_begin_transaction(self),
            Self::Commit { .. } => operators::control_flow::stop_commit(self),
            Self::Rollback { .. } => operators::control_flow::stop_rollback(self),
            Self::ShowStats { .. } => operators::control_flow::stop_show_stats(self),
            // Analysis & Migration
            Self::Analyze { .. } => operators::management::stop_analyze(self),
            Self::Migrate { .. } => operators::management::stop_migrate(self),
        };
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("stop", elapsed);
        result
    }

    /// Close the executor (clean up resources)
    pub fn close(&mut self) -> Result<(), QueryError> {
        let start = Instant::now();
        let result = match self {
            // Access operations
            Self::Start { .. } => operators::access::close_start(self),
            Self::GetVertices { .. } => operators::access::close_getvertices(self),
            Self::GetEdges { .. } => operators::access::close_getedges(self),
            Self::GetNeighbors { .. } => operators::access::close_getneighbors(self),
            Self::IndexScan { .. } => operators::access::close_indexscan(self),
            Self::EdgeIndexScan { .. } => operators::access::close_edgeindexscan(self),
            Self::Argument { .. } => operators::access::close_argument(self),
            Self::Sample { .. } => operators::access::close_sample(self),
            // Property & Index lookup
            Self::GetProp { .. } => operators::access::close_getprop(self),
            Self::LookupIndex { .. } => operators::access::close_lookupindex(self),
            // Data source operations
            Self::ScanVertices { .. } | Self::StorageScanVertices { .. } => {
                operators::sources::close_scanvertices(self)
            }
            Self::ScanEdges { .. } | Self::StorageScanEdges { .. } => {
                operators::sources::close_scanedges(self)
            }
            // Single input operations
            Self::Filter { .. } => operators::single_input::close_filter(self),
            Self::Project { .. } => operators::single_input::close_project(self),
            Self::Limit { .. } => operators::single_input::close_limit(self),
            Self::Distinct { .. } => operators::single_input::close_distinct(self),
            // Stateful operations
            Self::Aggregate { .. } => operators::stateful::close_aggregate(self),
            Self::Sort { .. } => operators::stateful::close_sort(self),
            Self::GroupBy { .. } => operators::stateful::close_groupby(self),
            Self::WindowFunction { .. } => operators::stateful::close_windowfunction(self),
            // Binary operations
            Self::HashJoin { .. } => operators::binary::close_hashjoin(self),
            Self::NestedLoopJoin { .. } => operators::binary::close_nestedloopjoin(self),
            Self::InnerJoin { .. } => operators::binary::close_innerjoin(self),
            Self::LeftJoin { .. } => operators::binary::close_leftjoin(self),
            Self::RightJoin { .. } => operators::binary::close_rightjoin(self),
            Self::FullOuterJoin { .. } => operators::binary::close_fullouterjoin(self),
            Self::CrossJoin { .. } => operators::binary::close_crossjoin(self),
            Self::SemiJoin { .. } => operators::binary::close_semijoin(self),
            // Set operations
            Self::Union { .. } => operators::set_ops::close_union(self),
            Self::UnionAll { .. } => operators::set_ops::close_unionall(self),
            Self::Intersect { .. } => operators::set_ops::close_intersect(self),
            Self::Except { .. } => operators::set_ops::close_except(self),
            // Relational operations
            Self::TopN { .. } => operators::relational::close_topn(self),
            Self::Dedup { .. } => operators::relational::close_dedup(self),
            Self::Assign { .. } => operators::relational::close_assign(self),
            Self::Materialize { .. } => operators::relational::close_materialize(self),
            Self::Remove { .. } => operators::relational::close_remove(self),
            Self::DataCollect { .. } => operators::relational::close_datacollect(self),
            Self::Unwind { .. } => operators::relational::close_unwind(self),
            Self::Apply { .. } => operators::relational::close_apply(self),
            Self::PatternApply { .. } => operators::relational::close_patternapply(self),
            Self::RollUpApply { .. } => operators::relational::close_rolluapply(self),
            Self::Minus { .. } => operators::relational::close_minus(self),
            Self::Window { .. } => operators::relational::close_window(self),
            // Data modification operations
            Self::InsertVertices { .. } => operators::data_modification::close_insertvertices(self),
            Self::InsertEdges { .. } => operators::data_modification::close_insertedges(self),
            Self::UpdateVertices { .. } => operators::data_modification::close_updatevertices(self),
            Self::UpdateEdges { .. } => operators::data_modification::close_updateedges(self),
            Self::DeleteVertices { .. } => operators::data_modification::close_deletevertices(self),
            Self::DeleteEdges { .. } => operators::data_modification::close_deleteedges(self),
            Self::PipeDeleteVertices { .. } => {
                operators::data_modification::close_pipedeletevertices(self)
            }
            Self::PipeDeleteEdges { .. } => {
                operators::data_modification::close_pipedeleteedges(self)
            }
            Self::DeleteTags { .. } => operators::data_modification::close_deletetags(self),
            // Graph traversal operations
            Self::Expand { .. } => operators::graph_traversal::close_expand(self),
            Self::ExpandAll { .. } => operators::graph_traversal::close_expandall(self),
            Self::Traverse { .. } => operators::graph_traversal::close_traverse(self),
            Self::TraverseAll { .. } => operators::graph_traversal::close_traverseall(self),
            Self::AppendVertices { .. } => operators::graph_traversal::close_appendvertices(self),
            Self::BiExpand { .. } => operators::graph_traversal::close_biexpand(self),
            Self::BiTraverse { .. } => operators::graph_traversal::close_bitraverse(self),
            Self::ShortestPath { .. } => operators::graph_traversal::close_shortestpath(self),
            Self::BFSShortest { .. } => operators::graph_traversal::close_bfsshortest(self),
            Self::AllPaths { .. } => operators::graph_traversal::close_allpaths(self),
            Self::MultiShortestPath { .. } => {
                operators::graph_traversal::close_multishortestpath(self)
            }
            Self::Subgraph { .. } => operators::graph_traversal::close_subgraph(self),
            // Search operations
            Self::FulltextSearch { .. } => operators::search::close_fulltext_search(self),
            Self::FulltextLookup { .. } => operators::search::close_fulltext_lookup(self),
            Self::MatchFulltext { .. } => operators::search::close_match_fulltext(self),
            Self::VectorSearch { .. } => operators::search::close_vector_search(self),
            Self::VectorLookup { .. } => operators::search::close_vector_lookup(self),
            Self::VectorMatch { .. } => operators::search::close_vector_match(self),
            // Management operations
            Self::SpaceManage { .. } => operators::management::close_space_manage(self),
            Self::TagManage { .. } => operators::management::close_tag_manage(self),
            Self::EdgeManage { .. } => operators::management::close_edge_manage(self),
            Self::IndexManage { .. } => operators::management::close_index_manage(self),
            Self::UserManage { .. } => operators::management::close_user_manage(self),
            Self::FulltextManage { .. } => operators::management::close_fulltext_manage(self),
            Self::VectorManage { .. } => operators::management::close_vector_manage(self),
            // Control flow operations
            Self::Loop { .. } => operators::control_flow::close_loop(self),
            Self::Select { .. } => operators::control_flow::close_select(self),
            Self::PassThrough { .. } => operators::control_flow::close_passthrough(self),
            Self::BeginTransaction { .. } => operators::control_flow::close_begin_transaction(self),
            Self::Commit { .. } => operators::control_flow::close_commit(self),
            Self::Rollback { .. } => operators::control_flow::close_rollback(self),
            Self::ShowStats { .. } => operators::control_flow::close_show_stats(self),
            // Analysis & Migration
            Self::Analyze { .. } => operators::management::close_analyze(self),
            Self::Migrate { .. } => operators::management::close_migrate(self),
        };
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("close", elapsed);
        let peak = self.peak_memory_bytes();
        if peak > 0 {
            self.record_profile_peak_memory(peak);
        }
        if let Some(rt) = self.get_runtime() {
            rt.release_resources();
        }
        result
    }
}

// ============ Spillable implementation (reserved) ============

impl Spillable for StreamingExecutor {
    fn spill_to_disk(&mut self) -> Result<(), QueryError> {
        Err(QueryError::execution(
            "Disk spill not yet implemented".to_string(),
        ))
    }

    fn spilled_size(&self) -> u64 {
        0
    }
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;

    fn create_test_buffer() -> Vec<Vec<Value>> {
        (0..100)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::String(format!("vertex_{}", i)),
                    Value::String(format!("label_{}", i % 10)),
                    Value::String(format!("prop_{}", i % 100)),
                    Value::BigInt((i % 1000) as i64),
                ]
            })
            .collect()
    }

    #[test]
    fn test_scan_vertices_with_buffer() {
        let buffer = create_test_buffer();
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: buffer.clone(),
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        executor.open().unwrap();
        let chunk = executor.advance().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 100);
        executor.close().unwrap();
    }

    #[test]
    fn test_scan_edges_with_buffer() {
        let buffer: Vec<Vec<Value>> = (0..50)
            .map(|i| {
                vec![
                    Value::BigInt((i % 1000) as i64),
                    Value::BigInt(((i + 1) % 1000) as i64),
                    Value::String(format!("edge_type_{}", i % 5)),
                    Value::BigInt((i % 100) as i64),
                    Value::BigInt((1000 + i) as i64),
                ]
            })
            .collect();

        let mut executor = StreamingExecutor::ScanEdges {
            partition_id: 0,
            buffer: buffer.clone(),
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        executor.open().unwrap();
        let chunk = executor.advance().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 50);
        executor.close().unwrap();
    }

    #[test]
    fn test_limit_executor() {
        let buffer = create_test_buffer();
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        });

        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 10,
            consumed: 0,
            opened: false,
            plan_node_id: 0,
            runtime: None,
        };

        limit.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = limit.advance().unwrap() {
            total += chunk.len();
        }
        limit.close().unwrap();

        assert_eq!(total, 10);
    }

    #[test]
    fn test_dynamic_column_count() {
        // Test with more than 5 columns to verify dynamic column support
        let buffer: Vec<Vec<Value>> = vec![
            vec![
                Value::BigInt(1),
                Value::String("a".to_string()),
                Value::String("b".to_string()),
                Value::String("c".to_string()),
                Value::String("d".to_string()),
                Value::String("e".to_string()),
                Value::String("f".to_string()),
                Value::String("g".to_string()),
                Value::String("h".to_string()),
            ],
            vec![
                Value::BigInt(2),
                Value::String("i".to_string()),
                Value::String("j".to_string()),
                Value::String("k".to_string()),
                Value::String("l".to_string()),
                Value::String("m".to_string()),
                Value::String("n".to_string()),
                Value::String("o".to_string()),
                Value::String("p".to_string()),
            ],
        ];

        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: buffer.clone(),
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        executor.open().unwrap();
        let chunk = executor.advance().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk.num_columns(), 9);

        executor.close().unwrap();
    }
}
