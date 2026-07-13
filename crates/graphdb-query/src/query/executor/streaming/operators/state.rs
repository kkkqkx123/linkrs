//! OperatorState: Mutable per-execution state for operator nodes.
//!
//! Created fresh for each query execution from an immutable
//! [`OperatorSpec`](super::spec::OperatorSpec).  Holds cursors,
//! buffers, hash tables, row iterators, and all other mutable data that
//! must NOT be shared across concurrent executions of the same cached plan.

use std::collections::{HashMap, HashSet};

use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::storage::cursor::{EdgeCursor, VertexCursor};

use super::super::chunk::DataChunk;
use super::super::executor::SortDirection;
use super::spec::SourceSpec;
use super::source_operator::NeighborScanState;

// ── Source state ─────────────────────────────────────────────────────────────

/// Mutable execution state for source operators.
#[derive(Debug)]
pub enum SourceState {
    ScanVertices {
        current_index: usize,
        col_names: Vec<String>,
    },
    StorageScanVertices {
        partition_id: usize,
        partition_range: Option<std::ops::Range<i64>>,
        cursor: Option<Box<dyn VertexCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    ScanEdges {
        current_index: usize,
        col_names: Vec<String>,
    },
    StorageScanEdges {
        partition_id: usize,
        partition_range: Option<std::ops::Range<i64>>,
        cursor: Option<Box<dyn EdgeCursor>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    GetVertices {
        position: usize,
    },
    GetEdges {
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    GetNeighbors {
        state: NeighborScanState,
    },
    EdgeIndexScan {
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    IndexScan {
        resolved_ids: Vec<Value>,
        position: usize,
    },
    Argument,
    GetProp,
    LookupIndex,
    Start,
}

impl SourceState {
    /// Create a fresh state for the given spec.
    pub fn from_spec(spec: &SourceSpec) -> Self {
        match spec {
            SourceSpec::ScanVertices { rows: _, col_names } => SourceState::ScanVertices {
                current_index: 0,
                col_names: col_names.clone(),
            },
            SourceSpec::StorageScanVertices { col_names, .. } => SourceState::StorageScanVertices {
                partition_id: 0,
                partition_range: None,
                cursor: None,
                buffer: Vec::new(),
                current_index: 0,
                col_names: col_names.clone(),
            },
            SourceSpec::ScanEdges { rows: _, col_names } => SourceState::ScanEdges {
                current_index: 0,
                col_names: col_names.clone(),
            },
            SourceSpec::StorageScanEdges { col_names, .. } => SourceState::StorageScanEdges {
                partition_id: 0,
                partition_range: None,
                cursor: None,
                buffer: Vec::new(),
                current_index: 0,
                col_names: col_names.clone(),
            },
            SourceSpec::GetVertices { .. } => SourceState::GetVertices { position: 0 },
            SourceSpec::GetEdges { .. } => SourceState::GetEdges { cursor: None },
            SourceSpec::GetNeighbors { .. } => SourceState::GetNeighbors {
                state: NeighborScanState::Init,
            },
            SourceSpec::EdgeIndexScan { .. } => SourceState::EdgeIndexScan { cursor: None },
            SourceSpec::IndexScan { .. } => SourceState::IndexScan {
                resolved_ids: Vec::new(),
                position: 0,
            },
            SourceSpec::Argument => SourceState::Argument,
            SourceSpec::GetProp { .. } => SourceState::GetProp,
            SourceSpec::LookupIndex { .. } => SourceState::LookupIndex,
            SourceSpec::Start => SourceState::Start,
        }
    }
}

// ── Unary state ──────────────────────────────────────────────────────────────

/// Mutable execution state for unary operators.
#[derive(Debug)]
pub enum UnaryState {
    Filter,
    Project,
    Limit {
        skipped: u32,
        consumed: u32,
    },
    Dedup {
        seen_rows: HashSet<Vec<Value>>,
    },
    Assign,
    Remove,
    Unwind {
        col_index: Option<usize>,
        all_rows: Vec<Vec<Value>>,
        current_row_index: usize,
        current_unwind_index: usize,
    },
    AppendVertices,
    Sample {
        consumed: u64,
    },
}

impl UnaryState {
    /// Create a fresh state for the given spec variant.
    pub fn from_spec_variant(spec_name: &str) -> Self {
        match spec_name {
            "Filter" => UnaryState::Filter,
            "Project" => UnaryState::Project,
            "Limit" => UnaryState::Limit {
                skipped: 0,
                consumed: 0,
            },
            "Dedup" => UnaryState::Dedup {
                seen_rows: HashSet::new(),
            },
            "Assign" => UnaryState::Assign,
            "Remove" => UnaryState::Remove,
            "Unwind" => UnaryState::Unwind {
                col_index: None,
                all_rows: Vec::new(),
                current_row_index: 0,
                current_unwind_index: 0,
            },
            "AppendVertices" => UnaryState::AppendVertices,
            "Sample" => UnaryState::Sample { consumed: 0 },
            _ => UnaryState::Filter,
        }
    }
}

// ── Blocking state ───────────────────────────────────────────────────────────

/// Re-export state structs from blocking_operator for use here.
pub use super::blocking_operator::{
    AggregateState, DataCollectState, DistinctState, FinalAggregateState, GroupByState,
    MaterializeState, PartialAggregateState, RollUpApplyState, SortState, TopNState,
    WindowFunctionState, WindowState,
};

/// Mutable execution state for blocking operators.
#[derive(Debug)]
pub enum BlockingState {
    Sort {
        memory_tracker: MemoryTracker,
        state: Option<SortState>,
    },
    Aggregate {
        memory_tracker: MemoryTracker,
        state: Option<AggregateState>,
    },
    GroupBy {
        memory_tracker: MemoryTracker,
        state: Option<GroupByState>,
    },
    WindowFunction {
        memory_tracker: MemoryTracker,
        state: Option<WindowFunctionState>,
    },
    Window {
        memory_tracker: MemoryTracker,
        state: Option<WindowState>,
    },
    TopN {
        memory_tracker: MemoryTracker,
        state: Option<TopNState>,
    },
    Distinct {
        memory_tracker: MemoryTracker,
        state: Option<DistinctState>,
    },
    Materialize {
        memory_tracker: MemoryTracker,
        state: Option<MaterializeState>,
    },
    DataCollect {
        memory_tracker: MemoryTracker,
        state: Option<DataCollectState>,
    },
    RollUpApply {
        memory_tracker: MemoryTracker,
        state: Option<RollUpApplyState>,
    },
    PartialAggregate {
        memory_tracker: MemoryTracker,
        state: Option<PartialAggregateState>,
    },
    FinalAggregate {
        memory_tracker: MemoryTracker,
        state: Option<FinalAggregateState>,
    },
}

// ── Join state ───────────────────────────────────────────────────────────────

/// Mutable execution state for join operators.
#[derive(Debug)]
pub enum JoinState {
    HashJoin {
        build_side_hash: HashMap<Vec<Value>, Vec<Vec<Value>>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    HashLeftJoin {
        build_side_hash: HashMap<Vec<Value>, Vec<Vec<Value>>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    NestedLoopJoin {
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    InnerJoin {
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    LeftJoin {
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    RightJoin {
        build_side_tuples: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    FullOuterJoin {
        left_rows: Vec<Vec<Value>>,
        right_rows: Vec<Vec<Value>>,
        matched_right_indices: HashSet<usize>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        phase: super::super::executor::FullOuterJoinPhase,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    CrossJoin {
        all_left_rows: Vec<Vec<Value>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    SemiJoin {
        right_rows: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
}

// ── Graph state ──────────────────────────────────────────────────────────────

/// Mutable execution state for graph traversal operators.
#[derive(Debug)]
pub enum GraphState {
    Expand,
    ExpandAll,
    Traverse {
        visited: HashSet<String>,
    },
    BiExpand,
    BiTraverse {
        visited: HashSet<String>,
    },
    ShortestPath,
    BFSShortest {
        frontier: Vec<Vec<Value>>,
        visited: HashSet<String>,
    },
    AllPaths {
        all_paths: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    },
    MultiShortestPath {
        all_paths: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    },
}

// ── Sink state ───────────────────────────────────────────────────────────────

/// Mutable execution state for sink (data modification) operators.
#[derive(Debug)]
pub enum SinkState {
    InsertVertices { rows_inserted: u64 },
    InsertEdges { rows_inserted: u64 },
    UpdateVertices { rows_updated: u64 },
    UpdateEdges { rows_updated: u64 },
    DeleteVertices { rows_deleted: u64 },
    DeleteEdges { rows_deleted: u64 },
    PipeDeleteVertices { rows_deleted: u64 },
    PipeDeleteEdges { rows_deleted: u64 },
    DeleteTags { rows_deleted: u64 },
}

// ── Exchange state ───────────────────────────────────────────────────────────

/// Mutable execution state for exchange (gather / merge / repartition) operators.
#[derive(Debug)]
pub enum ExchangeState {
    Concatenate {
        current_index: usize,
        col_names: Option<Vec<String>>,
    },
    MergeSort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        inputs: Vec<MergeInputState>,
        col_names: Option<Vec<String>>,
        limit: Option<usize>,
        emitted: usize,
    },
}

/// Internal merge cursor state for Exchange merge-sort.
#[derive(Debug)]
pub enum MergeInputState {
    Pending,
    Buffered { chunk: DataChunk, row_index: usize },
    Exhausted,
}

// ── Set state ────────────────────────────────────────────────────────────────

/// Mutable execution state for set operators.
#[derive(Debug)]
pub enum SetState {
    Union {
        seen_rows: HashSet<String>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
    },
    UnionAll {
        left_consumed: bool,
    },
    Intersect {
        left_rows: Vec<Vec<Value>>,
        right_rows: HashSet<String>,
        left_buffered: bool,
        right_buffered: bool,
        memory_tracker: MemoryTracker,
    },
    Except {
        exclude_rows: HashSet<String>,
        right_buffered: bool,
        memory_tracker: MemoryTracker,
    },
    Minus {
        exclude_rows: HashSet<String>,
        right_buffered: bool,
        memory_tracker: MemoryTracker,
    },
}

// ── Apply state ──────────────────────────────────────────────────────────────

/// Mutable execution state for apply operators.
#[derive(Debug)]
pub enum ApplyState {
    Apply,
    PatternApply {
        all_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        memory_tracker: MemoryTracker,
    },
    RollUpApply,
}

// ── DDL state ────────────────────────────────────────────────────────────────

/// Mutable execution state for DDL operators (minimal — DDL has no cursor/accumulator).
#[derive(Debug)]
pub enum DdlState {
    SpaceManage,
    TagManage,
    EdgeManage,
    IndexManage,
    UserManage,
    ShowStats,
    Analyze,
    Migrate,
}

impl DdlState {
    pub fn from_spec(spec: &super::spec::DdlSpec) -> Self {
        match spec {
            super::spec::DdlSpec::SpaceManage { .. } => DdlState::SpaceManage,
            super::spec::DdlSpec::TagManage { .. } => DdlState::TagManage,
            super::spec::DdlSpec::EdgeManage { .. } => DdlState::EdgeManage,
            super::spec::DdlSpec::IndexManage { .. } => DdlState::IndexManage,
            super::spec::DdlSpec::DeleteIndex { .. } => DdlState::IndexManage,
            super::spec::DdlSpec::UserManage { .. } => DdlState::UserManage,
            super::spec::DdlSpec::ShowStats { .. } => DdlState::ShowStats,
            super::spec::DdlSpec::Analyze { .. } => DdlState::Analyze,
            super::spec::DdlSpec::Migrate { .. } => DdlState::Migrate,
        }
    }
}

// ── Fulltext state ───────────────────────────────────────────────────────────

/// Mutable execution state for fulltext operators.
#[derive(Debug)]
pub enum FulltextState {
    FulltextManage,
    FulltextSearch,
    FulltextLookup,
    MatchFulltext,
}

impl FulltextState {
    pub fn from_spec(spec: &super::spec::FulltextSpec) -> Self {
        match spec {
            super::spec::FulltextSpec::FulltextManage { .. } => {
                FulltextState::FulltextManage
            }
            super::spec::FulltextSpec::FulltextSearch { .. } => {
                FulltextState::FulltextSearch
            }
            super::spec::FulltextSpec::FulltextLookup { .. } => {
                FulltextState::FulltextLookup
            }
            super::spec::FulltextSpec::MatchFulltext { .. } => {
                FulltextState::MatchFulltext
            }
        }
    }
}

// ── Vector state ─────────────────────────────────────────────────────────────

/// Mutable execution state for vector search operators.
#[derive(Debug)]
pub enum VectorState {
    VectorManage,
    VectorSearch,
    VectorLookup,
    VectorMatch,
}

impl VectorState {
    pub fn from_spec(spec: &super::spec::VectorSpec) -> Self {
        match spec {
            super::spec::VectorSpec::VectorManage { .. } => VectorState::VectorManage,
            super::spec::VectorSpec::VectorSearch { .. } => VectorState::VectorSearch,
            super::spec::VectorSpec::VectorLookup { .. } => VectorState::VectorLookup,
            super::spec::VectorSpec::VectorMatch { .. } => VectorState::VectorMatch,
        }
    }
}

// ── Txn state ────────────────────────────────────────────────────────────────

/// Mutable execution state for transaction operators.
#[derive(Debug)]
pub enum TxnState {
    BeginTransaction,
    Commit,
    Rollback,
}

impl TxnState {
    pub fn from_spec(spec: &super::spec::TxnSpec) -> Self {
        match spec {
            super::spec::TxnSpec::BeginTransaction { .. } => TxnState::BeginTransaction,
            super::spec::TxnSpec::Commit { .. } => TxnState::Commit,
            super::spec::TxnSpec::Rollback { .. } => TxnState::Rollback,
        }
    }
}
