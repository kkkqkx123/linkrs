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
//! Note: Phase 2c-1 expanded from 16 to 79 variants with stub implementations.
//! Actual implementations will be added incrementally in Phase 2c-2.

use parking_lot::RwLock;
use std::sync::Arc;

use super::chunk::DataChunk;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::Value;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
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
    },

    /// Scan edges from a partition
    /// Input data is pre-loaded into buffer (from storage layer)
    ScanEdges {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
    },

    // ============ Single Input ============
    /// Filter executor with expression-based predicates
    Filter {
        input: Box<StreamingExecutor>,
        predicate: Expression,
        opened: bool,
    },

    /// Project executor with expression-based column selection
    Project {
        input: Box<StreamingExecutor>,
        output_expressions: Vec<Expression>,
        opened: bool,
    },

    /// Limit executor
    Limit {
        input: Box<StreamingExecutor>,
        limit: u32,
        consumed: u32,
        opened: bool,
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
        /// Hash table: stringified row key -> matching right rows
        build_side_hash: std::collections::HashMap<String, Vec<Vec<Value>>>,
        /// All right rows (for Cartesian product when no join_condition)
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        opened: bool,
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
    },

    /// Distinct executor to eliminate duplicate rows
    Distinct {
        input: Box<StreamingExecutor>,
        /// Set of already-seen rows (as serialized strings)
        seen_rows: std::collections::HashSet<String>,
        opened: bool,
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
    },

    /// Set Union operation (combines all rows from left and right)
    Union {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Already-seen rows to eliminate duplicates
        seen_rows: std::collections::HashSet<String>,
        left_consumed: bool,
        opened: bool,
    },

    /// Set UnionAll operation (combines all rows without deduplication)
    UnionAll {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        left_consumed: bool,
        opened: bool,
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
    },

    /// Set Except/Minus operation (returns rows from left not in right)
    Except {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Rows to exclude (from right side)
        exclude_rows: std::collections::HashSet<String>,
        right_buffered: bool,
        opened: bool,
    },

    // ============ Access Operations ============
    Start {
        opened: bool,
    },
    GetVertices {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        opened: bool,
    },
    GetEdges {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
        opened: bool,
    },
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        direction: String,
        opened: bool,
    },
    EdgeIndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: Option<String>,
        opened: bool,
    },
    IndexScan {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: Option<String>,
        index_value: Option<Value>,
        opened: bool,
    },
    Argument {
        opened: bool,
    },
    Sample {
        opened: bool,
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
    },

    /// Lookup vertices by index key
    LookupIndex {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: String,
        index_condition: Option<(String, Value)>,
        limit: Option<usize>,
        opened: bool,
    },

    // ============ Join Operations (stub) ============
    InnerJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        opened: bool,
    },
    LeftJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        opened: bool,
    },
    RightJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        right_consumed: bool,
        opened: bool,
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
        opened: bool,
    },
    CrossJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        all_left_rows: Vec<Vec<Value>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        right_consumed: bool,
        opened: bool,
    },
    SemiJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        join_condition: Option<Expression>,
        right_rows: Vec<Vec<Value>>,
        right_consumed: bool,
        opened: bool,
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
    },

    ExpandAll {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: String,
        direction: String,
        filter_expr: Option<Expression>,
        opened: bool,
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
    },

    AppendVertices {
        input: Box<StreamingExecutor>,
        vertex_properties: Vec<(String, Expression)>,
        opened: bool,
    },

    BiExpand {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_type: String,
        opened: bool,
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
    },

    ShortestPath {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_type: String,
        direction: String,
        opened: bool,
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
    },

    UpdateVertices {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        updates: Vec<(String, Expression)>,
        rows_updated: u64,
        opened: bool,
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
    },

    DeleteVertices {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_id_col: String,
        rows_deleted: u64,
        opened: bool,
    },

    DeleteEdges {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        rows_deleted: u64,
        opened: bool,
    },

    PipeDeleteVertices {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        vertex_id_col: String,
        rows_deleted: u64,
        opened: bool,
    },

    PipeDeleteEdges {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        src_col: String,
        dst_col: String,
        rows_deleted: u64,
        opened: bool,
    },

    DeleteTags {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        tag_names: Vec<String>,
        vertex_ids: Option<Vec<Value>>,
        rows_deleted: u64,
        opened: bool,
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
    },
    SpaceManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        action: String,
        space_name: Option<String>,
        opened: bool,
    },

    TagManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        tag_name: Option<String>,
        opened: bool,
    },

    EdgeManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        edge_type: Option<String>,
        opened: bool,
    },

    IndexManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        index_name: Option<String>,
        opened: bool,
    },

    UserManage {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        action: String,
        username: Option<String>,
        opened: bool,
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
    },

    Dedup {
        input: Box<StreamingExecutor>,
        seen_rows: std::collections::HashSet<String>,
        opened: bool,
    },

    Assign {
        input: Box<StreamingExecutor>,
        assignments: Vec<(String, Expression)>,
        opened: bool,
    },

    Materialize {
        input: Box<StreamingExecutor>,
        materialized_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        materialized: bool,
        opened: bool,
    },

    Remove {
        input: Box<StreamingExecutor>,
        columns_to_remove: Vec<String>,
        opened: bool,
    },

    DataCollect {
        input: Box<StreamingExecutor>,
        all_rows: Vec<Vec<Value>>,
        emitted: bool,
        opened: bool,
    },

    Unwind {
        input: Box<StreamingExecutor>,
        unwind_column: String,
        col_index: Option<usize>,
        all_rows: Vec<Vec<Value>>,
        current_row_index: usize,
        current_unwind_index: usize,
        opened: bool,
    },

    Apply {
        input: Box<StreamingExecutor>,
        apply_expression: Expression,
        opened: bool,
    },

    PatternApply {
        input: Box<StreamingExecutor>,
        pattern: Expression,
        all_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
    },

    RollUpApply {
        input: Box<StreamingExecutor>,
        rollup_expressions: Vec<Expression>,
        all_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
    },

    Minus {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        exclude_rows: std::collections::HashSet<String>,
        right_buffered: bool,
        opened: bool,
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
    },

    // ============ Control Flow ============
    Loop {
        input: Box<StreamingExecutor>,
        condition: Option<String>,
        opened: bool,
    },

    Select {
        input: Box<StreamingExecutor>,
        selection_expr: Option<String>,
        opened: bool,
    },

    PassThrough {
        input: Box<StreamingExecutor>,
        opened: bool,
    },

    BeginTransaction {
        input: Box<StreamingExecutor>,
        transaction_id: Option<String>,
        opened: bool,
    },

    Commit {
        input: Box<StreamingExecutor>,
        transaction_id: Option<String>,
        opened: bool,
    },

    Rollback {
        input: Box<StreamingExecutor>,
        transaction_id: Option<String>,
        opened: bool,
    },

    // ============ Other (stub) ============
    ShowStats {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        opened: bool,
    },

    // ============ Analysis & Migration ============
    Analyze {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        analyze_target: String,
        target_name: Option<String>,
        opened: bool,
    },

    Migrate {
        input: Box<StreamingExecutor>,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        action: String,
        migration_data: Option<String>,
        opened: bool,
    },
}

impl StreamingExecutor {
    /// Initialize the executor
    pub fn open(&mut self) -> Result<(), QueryError> {
        match self {
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
            Self::ScanVertices { .. } => operators::sources::open_scanvertices(self),
            Self::ScanEdges { .. } => operators::sources::open_scanedges(self),
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
        }
    }

    /// Pull next chunk from the executor
    pub fn next(&mut self) -> Result<Option<DataChunk>, QueryError> {
        match self {
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
            Self::ScanVertices { .. } => operators::sources::next_scanvertices(self),
            Self::ScanEdges { .. } => operators::sources::next_scanedges(self),
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
        }
    }

    /// Stop the executor (signal no more input needed)
    pub fn stop(&mut self) -> Result<(), QueryError> {
        match self {
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
            Self::ScanVertices { .. } => operators::sources::stop_scanvertices(self),
            Self::ScanEdges { .. } => operators::sources::stop_scanedges(self),
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
        }
    }

    /// Close the executor (clean up resources)
    pub fn close(&mut self) -> Result<(), QueryError> {
        match self {
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
            Self::ScanVertices { .. } => operators::sources::close_scanvertices(self),
            Self::ScanEdges { .. } => operators::sources::close_scanedges(self),
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
        }
    }
}

#[cfg(test)]
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
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
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
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
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
        });

        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 10,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = limit.next().unwrap() {
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
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk.num_columns(), 9);

        executor.close().unwrap();
    }
}
