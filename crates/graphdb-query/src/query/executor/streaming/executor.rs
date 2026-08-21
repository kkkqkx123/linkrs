//! StreamingExecutor: Thin dispatch layer over domain-specific operator enums

use std::sync::Arc;
use std::time::Instant;

use super::chunk::DataChunk;
use super::runtime::{ExecutionRuntime, OperatorProfile, OperatorProfileKey};
use super::slot::SlotLayout;
use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::{MemoryTracker, Spillable};

pub use super::context::ValueRowContext;
pub use super::helpers::{comparison, conversion};
pub use super::operators::base::OperatorBase;
use super::operators::base::OperatorLifecycle;
use super::operators::source_operator::OperatorConfig;
use super::operators::state::ExchangeState;

use super::operators::apply_operator::ApplyOperator;
use super::operators::apply_operator::ApplyOperatorKind;
use super::operators::blocking::BlockingOperator;
use super::operators::blocking::BlockingOperatorKind;
use super::operators::ddl_operator::DdlOperator;
use super::operators::ddl_operator::DdlOperatorKind;
use super::operators::exchange_operator::ExchangeOperator;
use super::operators::fulltext_operator::FulltextOperator;
use super::operators::fulltext_operator::FulltextOperatorKind;
use super::operators::gather_operator::GatherOperator;
use super::operators::gather_operator::GatherOperatorKind;
use super::operators::graph_operator::GraphOperator;
use super::operators::graph_operator::GraphOperatorKind;
use super::operators::join_operator::JoinOperator;
use super::operators::join_operator::JoinOperatorKind;
use super::operators::recursive_fragment_operator::RecursiveFragmentOperator;
use super::operators::recursive_fragment_operator::RecursiveFragmentOperatorKind;
use super::operators::set_operator::SetOperator;
use super::operators::set_operator::SetOperatorKind;
use super::operators::shuffle_join_operator::HashShuffleJoinOperator;
use super::operators::sink_operator::SinkOperator;
use super::operators::sink_operator::SinkOperatorKind;
use super::operators::source_operator::SourceOperator;
use super::operators::source_operator::SourceOperatorKind;
use super::operators::txn_operator::TxnOperator;
use super::operators::txn_operator::TxnOperatorKind;
use super::operators::unary_operator::UnaryOperator;
use super::operators::unary_operator::UnaryOperatorKind;
use super::operators::vector_operator::VectorOperator;
use super::operators::vector_operator::VectorOperatorKind;

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

/// StreamingExecutor: 16-variant dispatch enum over domain-specific operators.
///
/// Each variant holds an OperatorBase (shared lifecycle/base fields), zero
/// or more child executors, and a domain-specific operator wrapper that
/// implements the per-operator lifecycle logic with zero-context methods.
///
/// Gather/Exchange take N children (Vec) and merge their output via
/// Concatenate or MergeSort mode. Exchange additionally uses the query-level
/// `MorselWorkerPool` for morsel-style dynamic partition execution.
#[derive(Debug)]
pub enum StreamingExecutor {
    Source(OperatorBase, SourceOperator),
    Unary(OperatorBase, Box<StreamingExecutor>, UnaryOperator),
    Join(
        OperatorBase,
        Box<StreamingExecutor>,
        Box<StreamingExecutor>,
        JoinOperator,
    ),
    Set(
        OperatorBase,
        Box<StreamingExecutor>,
        Box<StreamingExecutor>,
        SetOperator,
    ),
    Apply(
        OperatorBase,
        Box<StreamingExecutor>,
        Box<StreamingExecutor>,
        ApplyOperator,
    ),
    Blocking(OperatorBase, Box<StreamingExecutor>, BlockingOperator),
    Graph(OperatorBase, Box<StreamingExecutor>, GraphOperator),
    /// M7: Recursive fragment for variable-length path traversal,
    /// BFS, shortest-path, and multi-round graph algorithms.
    RecursiveFragment(
        OperatorBase,
        Box<StreamingExecutor>,
        RecursiveFragmentOperator,
    ),
    Sink(OperatorBase, Box<StreamingExecutor>, SinkOperator),
    Ddl(OperatorBase, Box<StreamingExecutor>, DdlOperator),
    Fulltext(OperatorBase, Box<StreamingExecutor>, FulltextOperator),
    Vector(OperatorBase, Box<StreamingExecutor>, VectorOperator),
    Txn(OperatorBase, Box<StreamingExecutor>, TxnOperator),
    Gather(OperatorBase, Vec<StreamingExecutor>, GatherOperator),
    /// Morsel-style exchange: gathers N partition outputs via Concatenate
    /// or MergeSort using the query-level `MorselWorkerPool`.
    Exchange(OperatorBase, Vec<StreamingExecutor>, ExchangeOperator),
    /// Hash repartition join: takes multiple left/right local trees,
    /// distributes rows by hash of join keys into buckets, joins per bucket.
    HashShuffleJoin(
        OperatorBase,
        Vec<StreamingExecutor>,
        Vec<StreamingExecutor>,
        HashShuffleJoinOperator,
    ),
}

/// Dispatch `open` (all operators open their children themselves).
macro_rules! dispatch_open {
    ($self:expr) => {
        match $self {
            Self::Source(_, op) => op.open(),
            Self::Unary(_, input, op) => op.open(input),
            Self::Join(_, left, right, op) => op.open(left, right),
            Self::Set(_, left, right, op) => op.open(left, right),
            Self::Apply(_, left, right, op) => op.open(left, right),
            Self::Blocking(_, input, op) => op.open(input),
            Self::Graph(_, input, op) => op.open(input),
            Self::RecursiveFragment(_, input, op) => op.open(input),
            Self::Sink(_, input, op) => op.open(input),
            Self::Ddl(_, input, op) => op.open(input),
            Self::Fulltext(_, input, op) => op.open(input),
            Self::Vector(_, input, op) => op.open(input),
            Self::Txn(_, input, op) => op.open(input),
            Self::Gather(_, children, op) => op.open(children),
            Self::Exchange(_, children, op) => op.open(children),
            Self::HashShuffleJoin(_, left, right, op) => op.open(left, right),
        }
    };
}

/// Dispatch `next` (same signature family as `open`).
macro_rules! dispatch_next {
    ($self:expr) => {
        match $self {
            Self::Source(_, op) => op.next(),
            Self::Unary(_, input, op) => op.next(input),
            Self::Join(_, left, right, op) => op.next(left, right),
            Self::Set(_, left, right, op) => op.next(left, right),
            Self::Apply(_, left, right, op) => op.next(left, right),
            Self::Blocking(_, input, op) => op.next(input),
            Self::Graph(_, input, op) => op.next(input),
            Self::RecursiveFragment(_, input, op) => op.next(input),
            Self::Sink(_, input, op) => op.next(input),
            Self::Ddl(_, input, op) => op.next(input),
            Self::Fulltext(_, input, op) => op.next(input),
            Self::Vector(_, input, op) => op.next(input),
            Self::Txn(_, input, op) => op.next(input),
            Self::Gather(_, children, op) => op.next(children),
            Self::Exchange(_, children, op) => op.next(children),
            Self::HashShuffleJoin(_, left, right, op) => op.next(left, right),
        }
    };
}

/// Dispatch `reset`. Blocking operators fully re-create their state in
/// `open()` and drop it in `close()`, so the close+open fallback is exact.
/// The remaining operators (Sink/Gather/Exchange/…/Txn) do not appear inside
/// resettable sub-plans; the fallback is a transitional audit point
/// (EXPLAIN `reset:fallback`).
macro_rules! dispatch_reset {
    ($self:expr) => {
        match $self {
            Self::Source(_, op) => op.reset(),
            Self::Unary(_, input, op) => op.reset(input),
            Self::Join(_, left, right, op) => op.reset(left, right),
            Self::Set(_, left, right, op) => op.reset(left, right),
            Self::Apply(_, left, right, op) => op.reset(left, right),
            Self::Graph(_, input, op) => op.reset(input),
            Self::RecursiveFragment(_, input, op) => op.reset(input),
            _ => $self.fallback_reset(),
        }
    };
}

/// Dispatch `stop`.
macro_rules! dispatch_stop {
    ($self:expr) => {
        match $self {
            Self::Source(_, op) => op.stop(),
            Self::Unary(_, _, op) => op.stop(),
            Self::Join(_, _, _, op) => op.stop(),
            Self::Set(_, _, _, op) => op.stop(),
            Self::Apply(_, _, _, op) => op.stop(),
            Self::Blocking(_, _, op) => op.stop(),
            Self::Graph(_, _, op) => op.stop(),
            Self::RecursiveFragment(_, _, op) => op.stop(),
            Self::Sink(_, _, op) => op.stop(),
            Self::Ddl(_, _, op) => op.stop(),
            Self::Fulltext(_, _, op) => op.stop(),
            Self::Vector(_, _, op) => op.stop(),
            Self::Txn(_, _, op) => op.stop(),
            Self::Gather(_, _, op) => op.stop(),
            Self::Exchange(_, _, op) => op.stop(),
            Self::HashShuffleJoin(_, _, _, op) => op.stop(),
        }
    };
}

/// Dispatch `close`.
macro_rules! dispatch_close {
    ($self:expr) => {
        match $self {
            Self::Source(_, op) => op.close(),
            Self::Unary(_, _, op) => op.close(),
            Self::Join(_, _, _, op) => op.close(),
            Self::Set(_, _, _, op) => op.close(),
            Self::Apply(_, _, _, op) => op.close(),
            Self::Blocking(_, _, op) => op.close(),
            Self::Graph(_, _, op) => op.close(),
            Self::RecursiveFragment(_, _, op) => op.close(),
            Self::Sink(_, _, op) => op.close(),
            Self::Ddl(_, _, op) => op.close(),
            Self::Fulltext(_, _, op) => op.close(),
            Self::Vector(_, _, op) => op.close(),
            Self::Txn(_, _, op) => op.close(),
            Self::Gather(_, _, op) => op.close(),
            Self::Exchange(_, _, op) => op.close(),
            Self::HashShuffleJoin(_, _, _, op) => op.close(),
        }
    };
}

impl StreamingExecutor {
    /// Recursively inject the runtime and execution config into every
    /// operator wrapper. `open()` derives the config from the base fields,
    /// so callers only need `set_runtime`/`set_chunk_size`/`set_partition_id`
    /// beforehand. Idempotent; safe to call on already-opened trees.
    pub fn inject_context(
        &mut self,
        runtime: Option<Arc<ExecutionRuntime>>,
        config: OperatorConfig,
    ) {
        let runtime_ref = runtime.as_ref();
        match self {
            Self::Source(_, op) => op.inject_context(runtime_ref, config),
            Self::Unary(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Join(_, _, _, op) => op.inject_context(runtime_ref, config),
            Self::Set(_, _, _, op) => op.inject_context(runtime_ref, config),
            Self::Apply(_, _, _, op) => op.inject_context(runtime_ref, config),
            Self::Blocking(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Graph(_, _, op) => op.inject_context(runtime_ref, config),
            Self::RecursiveFragment(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Sink(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Ddl(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Fulltext(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Vector(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Txn(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Gather(_, _, op) => op.inject_context(runtime_ref, config),
            Self::Exchange(_, _, op) => op.inject_context(runtime_ref, config),
            Self::HashShuffleJoin(_, _, _, op) => op.inject_context(runtime_ref, config),
        }
        for child in self.children_mut() {
            child.inject_context(runtime.clone(), config);
        }
    }

    /// Recursively set the runtime on this operator and all children.
    pub fn set_runtime(&mut self, rt: Option<Arc<ExecutionRuntime>>) {
        if let (Self::Graph(_, _, operator), Some(runtime)) = (&mut *self, rt.as_ref()) {
            operator.bind_runtime(runtime);
        }
        if let (Self::RecursiveFragment(_, _, operator), Some(runtime)) = (&mut *self, rt.as_ref())
        {
            operator.bind_runtime(runtime);
        }
        self.base_mut().runtime = rt.clone();
        for child in self.children_mut() {
            child.set_runtime(rt.clone());
        }
    }

    /// Recursively set the chunk size on this operator and all children.
    pub fn set_chunk_size(&mut self, chunk_size: usize) {
        self.base_mut().chunk_size = chunk_size;
        for child in self.children_mut() {
            child.set_chunk_size(chunk_size);
        }
    }

    /// Return the plan node ID of this operator.
    pub fn plan_node_id(&self) -> i64 {
        self.base().plan_node_id
    }

    /// Return the unique profile key for this concrete operator instance.
    pub fn profile_key(&self) -> OperatorProfileKey {
        self.base().profile_key()
    }

    /// Recursively mark this executor tree as belonging to a local partition.
    pub fn set_partition_id(&mut self, partition_id: usize) {
        self.base_mut().partition_id = Some(partition_id);
        self.base_mut().is_global = false;
        for child in self.children_mut() {
            child.set_partition_id(partition_id);
        }
    }

    /// Recursively mark an executor tree as global. This is applied before a
    /// gather/exchange node is attached, so the local children retain their
    /// own partition identifiers.
    pub fn set_global(&mut self) {
        self.base_mut().partition_id = None;
        self.base_mut().is_global = true;
        for child in self.children_mut() {
            child.set_global();
        }
    }

    /// Whether this tree may be executed independently for each partition and
    /// concatenated without changing query semantics.
    ///
    /// Global operators are intentionally excluded. They require a dedicated
    /// physical split (for example local sort plus merge sort), not a copied
    /// copy of the original logical tree.
    pub fn is_partition_local(&self) -> bool {
        match self {
            Self::Source(_, op) => matches!(
                &op.kind,
                SourceOperatorKind::ScanVertices { .. }
                    | SourceOperatorKind::StorageScanVertices { .. }
                    | SourceOperatorKind::ScanEdges { .. }
                    | SourceOperatorKind::StorageScanEdges { .. }
            ),
            Self::Unary(_, input, op) => {
                matches!(
                    &op.kind,
                    UnaryOperatorKind::Filter { .. }
                        | UnaryOperatorKind::Project { .. }
                        | UnaryOperatorKind::Assign { .. }
                        | UnaryOperatorKind::Remove { .. }
                        | UnaryOperatorKind::Unwind { .. }
                        | UnaryOperatorKind::AppendVertices { .. }
                ) && input.is_partition_local()
            }
            Self::Blocking(_, input, op) => {
                matches!(
                    &op.kind,
                    BlockingOperatorKind::PartialAggregate { .. }
                        | BlockingOperatorKind::Distinct { .. }
                        | BlockingOperatorKind::TopN { .. }
                ) && input.is_partition_local()
            }
            Self::HashShuffleJoin(..) | Self::Exchange(..) | Self::RecursiveFragment(..) => false,
            _ => false,
        }
    }

    /// Access the runtime reference, if attached.
    pub fn get_runtime(&self) -> Option<&ExecutionRuntime> {
        self.base().runtime.as_deref()
    }

    /// Check cancellation via the attached runtime.
    pub fn ensure_not_cancelled(&self) -> Result<(), QueryError> {
        self.base().ensure_not_cancelled()
    }

    /// Profile name for this operator variant.
    pub fn operator_name(&self) -> &'static str {
        use StreamingExecutor::*;
        match self {
            Source(_, op) => match &op.kind {
                SourceOperatorKind::ScanVertices { .. }
                | SourceOperatorKind::StorageScanVertices { .. }
                | SourceOperatorKind::StandaloneValues { .. } => "ScanVertices",
                SourceOperatorKind::ScanEdges { .. }
                | SourceOperatorKind::StorageScanEdges { .. } => "ScanEdges",
                SourceOperatorKind::GetVertices { .. } => "GetVertices",
                SourceOperatorKind::GetEdges { .. } => "GetEdges",
                SourceOperatorKind::GetNeighbors { .. } => "GetNeighbors",
                SourceOperatorKind::IndexScan { .. } => "IndexScan",
                SourceOperatorKind::Argument => "Argument",
                SourceOperatorKind::GetProp { .. } => "GetProp",
                SourceOperatorKind::Start => "Start",
            },
            Unary(_, _, op) => match &op.kind {
                UnaryOperatorKind::Filter { .. } => "Filter",
                UnaryOperatorKind::Project { .. } => "Project",
                UnaryOperatorKind::Limit { .. } => "Limit",
                UnaryOperatorKind::Dedup { .. } => "Dedup",
                UnaryOperatorKind::Assign { .. } => "Assign",
                UnaryOperatorKind::Remove { .. } => "Remove",
                UnaryOperatorKind::Unwind { .. } => "Unwind",
                UnaryOperatorKind::AppendVertices { .. } => "AppendVertices",
                UnaryOperatorKind::Sample { .. } => "Sample",
            },
            Txn(_, _, op) => match &op.kind {
                TxnOperatorKind::BeginTransaction { .. } => "BeginTransaction",
                TxnOperatorKind::Commit { .. } => "Commit",
                TxnOperatorKind::Rollback { .. } => "Rollback",
                TxnOperatorKind::RollbackToSavepoint { .. } => "RollbackToSavepoint",
                TxnOperatorKind::Savepoint { .. } => "Savepoint",
                TxnOperatorKind::ReleaseSavepoint { .. } => "ReleaseSavepoint",
            },
            Join(_, _, _, op) => match &op.kind {
                JoinOperatorKind::HashJoin { .. } => "HashJoin",
                JoinOperatorKind::HashLeftJoin { .. } => "HashLeftJoin",
                JoinOperatorKind::NestedLoopJoin { .. } => "NestedLoopJoin",
                JoinOperatorKind::InnerJoin { .. } => "InnerJoin",
                JoinOperatorKind::LeftJoin { .. } => "LeftJoin",
                JoinOperatorKind::RightJoin { .. } => "RightJoin",
                JoinOperatorKind::FullOuterJoin { .. } => "FullOuterJoin",
                JoinOperatorKind::CrossJoin { .. } => "CrossJoin",
                JoinOperatorKind::SemiJoin { .. } => "SemiJoin",
            },
            Set(_, _, _, op) => match &op.kind {
                SetOperatorKind::Union { .. } => "Union",
                SetOperatorKind::UnionAll { .. } => "UnionAll",
                SetOperatorKind::Intersect { .. } => "Intersect",
                SetOperatorKind::Except { .. } => "Except",
                SetOperatorKind::Minus { .. } => "Minus",
            },
            Apply(_, _, _, op) => match &op.kind {
                ApplyOperatorKind::Apply { .. } => "Apply",
                ApplyOperatorKind::PatternApply { .. } => "PatternApply",
                ApplyOperatorKind::CorrelatedApply { .. } => "CorrelatedApply",
                ApplyOperatorKind::RollUpApply { .. } => "RollUpApply",
            },
            Blocking(_, _, op) => match &op.kind {
                BlockingOperatorKind::Sort { .. } => "Sort",
                BlockingOperatorKind::Aggregate { .. } => "Aggregate",
                BlockingOperatorKind::GroupBy { .. } => "GroupBy",
                BlockingOperatorKind::WindowFunction { .. } => "WindowFunction",
                BlockingOperatorKind::Window { .. } => "Window",
                BlockingOperatorKind::TopN { .. } => "TopN",
                BlockingOperatorKind::Distinct { .. } => "Distinct",
                BlockingOperatorKind::Materialize { .. } => "Materialize",
                BlockingOperatorKind::DataCollect { .. } => "DataCollect",
                BlockingOperatorKind::RollUpApply { .. } => "RollUpApply",
                BlockingOperatorKind::PartialAggregate { .. } => "PartialAggregate",
                BlockingOperatorKind::FinalAggregate { .. } => "FinalAggregate",
            },
            Graph(_, _, op) => match &op.kind {
                GraphOperatorKind::Expand { .. } => "Expand",
                GraphOperatorKind::ExpandAll { .. } => "ExpandAll",
                GraphOperatorKind::Traverse { .. } => "Traverse",
                GraphOperatorKind::TraverseAll { .. } => "TraverseAll",
                GraphOperatorKind::BiExpand { .. } => "BiExpand",
                GraphOperatorKind::BiTraverse { .. } => "BiTraverse",
                GraphOperatorKind::Subgraph { .. } => "Subgraph",
            },
            RecursiveFragment(_, _, op) => match &op.kind {
                RecursiveFragmentOperatorKind::ShortestPath { .. } => "RecursiveShortestPath",
                RecursiveFragmentOperatorKind::MultiShortestPath { .. } => {
                    "RecursiveMultiShortestPath"
                }
                RecursiveFragmentOperatorKind::BFSShortest { .. } => "RecursiveBFSShortest",
                RecursiveFragmentOperatorKind::AllPaths { .. } => "RecursiveAllPaths",
            },
            Sink(_, _, op) => match &op.kind {
                SinkOperatorKind::CopyFrom { .. } => "CopyFrom",
                SinkOperatorKind::InsertVertices { .. } => "InsertVertices",
                SinkOperatorKind::InsertEdges { .. } => "InsertEdges",
                SinkOperatorKind::UpdateVertices { .. } => "UpdateVertices",
                SinkOperatorKind::UpdateEdges { .. } => "UpdateEdges",
                SinkOperatorKind::DeleteVertices { .. } => "DeleteVertices",
                SinkOperatorKind::DeleteEdges { .. } => "DeleteEdges",
                SinkOperatorKind::PipeDeleteVertices { .. } => "PipeDeleteVertices",
                SinkOperatorKind::PipeDeleteEdges { .. } => "PipeDeleteEdges",
                SinkOperatorKind::DeleteTags { .. } => "DeleteTags",
            },
            Ddl(_, _, op) => match &op.kind {
                DdlOperatorKind::SpaceManage { .. } => "SpaceManage",
                DdlOperatorKind::TagManage { .. } => "TagManage",
                DdlOperatorKind::EdgeManage { .. } => "EdgeManage",
                DdlOperatorKind::IndexManage { .. } => "IndexManage",
                DdlOperatorKind::DeleteIndex { .. } => "DeleteIndex",
                DdlOperatorKind::UserManage { .. } => "UserManage",
                DdlOperatorKind::ShowStats { .. } => "ShowStats",
                DdlOperatorKind::ShowConfigs { .. } => "ShowConfigs",
                DdlOperatorKind::ShowQueries { .. } => "ShowQueries",
                DdlOperatorKind::ShowSessions { .. } => "ShowSessions",
                DdlOperatorKind::Analyze { .. } => "Analyze",
                DdlOperatorKind::Migrate { .. } => "Migrate",
            },
            Fulltext(_, _, op) => match &op.kind {
                FulltextOperatorKind::FulltextManage { .. } => "FulltextManage",
                FulltextOperatorKind::FulltextSearch { .. } => "FulltextSearch",
                FulltextOperatorKind::FulltextLookup { .. } => "FulltextLookup",
                FulltextOperatorKind::MatchFulltext { .. } => "MatchFulltext",
            },
            Vector(_, _, op) => match &op.kind {
                VectorOperatorKind::VectorManage { .. } => "VectorManage",
                VectorOperatorKind::VectorSearch { .. } => "VectorSearch",
                VectorOperatorKind::VectorLookup { .. } => "VectorLookup",
                VectorOperatorKind::VectorMatch { .. } => "VectorMatch",
            },
            Gather(_, _, op) => match &op.kind {
                GatherOperatorKind::Concatenate { .. } => "Gather(Concatenate)",
                GatherOperatorKind::MergeSort { .. } => "Gather(MergeSort)",
            },
            Exchange(_, _, op) => match &op.state {
                ExchangeState::Concatenate { .. } => "Exchange(Concatenate)",
                ExchangeState::MergeSort { .. } => "Exchange(MergeSort)",
                ExchangeState::RepartitionHash { .. } => "Exchange(RepartitionHash)",
                ExchangeState::Broadcast { .. } => "Exchange(Broadcast)",
                ExchangeState::Barrier { .. } => "Exchange(Barrier)",
                ExchangeState::Materialize { .. } => "Exchange(Materialize)",
            },
            HashShuffleJoin(_, _, _, op) => match op.join_kind {
                super::operators::shuffle_join_operator::HashJoinKind::Inner => {
                    "HashShuffleJoin(Inner)"
                }
                super::operators::shuffle_join_operator::HashJoinKind::Left => {
                    "HashShuffleJoin(Left)"
                }
            },
        }
    }

    /// Record profile timing for this operator, using the correct operator name.
    ///
    /// Fast path: looks up the pre-registered [`ProfileEntry`] via read-lock,
    /// then updates the atomic counter without holding any lock.
    pub fn record_profile_timing(&self, phase: &str, elapsed_us: u64) {
        let Some(rt) = &self.base().runtime else {
            return;
        };
        let entry = rt.profile().get_entry(&self.profile_key());
        let entry = entry.unwrap_or_else(|| {
            // First access — register on demand (rare: tests that bypass open())
            let name = self.operator_name();
            let op = OperatorProfile {
                physical_operator_id: self.base().physical_operator_id,
                node_id: self.plan_node_id(),
                partition_id: self.base().partition_id,
                name: name.to_string(),
                ..OperatorProfile::default()
            };
            rt.register_operator(&op)
        });
        let ordering = std::sync::atomic::Ordering::Relaxed;
        if phase == "open" {
            entry.open_time_us.fetch_add(elapsed_us, ordering);
        } else if phase == "next" {
            entry.next_time_us.fetch_add(elapsed_us, ordering);
        } else if phase == "close" {
            entry.close_time_us.fetch_add(elapsed_us, ordering);
        }
    }

    /// Get peak memory from the memory_tracker, if this operator has one.
    pub fn peak_memory_bytes(&self) -> u64 {
        self.memory_tracker().map_or(0, |mt| mt.peak() as u64)
    }

    /// Return the parallel fallback reason from any operator in this
    /// tree that has a recorded reason, or `None`.
    pub fn parallel_fallback_reason(&self) -> Option<String> {
        match self {
            Self::Gather(_, children, _) | Self::Exchange(_, children, _) => {
                children.iter().find_map(|c| c.parallel_fallback_reason())
            }
            Self::Unary(_, child, _)
            | Self::Blocking(_, child, _)
            | Self::Graph(_, child, _)
            | Self::RecursiveFragment(_, child, _)
            | Self::Sink(_, child, _)
            | Self::Ddl(_, child, _)
            | Self::Fulltext(_, child, _)
            | Self::Vector(_, child, _)
            | Self::Txn(_, child, _) => child.parallel_fallback_reason(),
            Self::Join(_, left, right, _) => left
                .parallel_fallback_reason()
                .or_else(|| right.parallel_fallback_reason()),
            Self::Set(_, left, right, _) => left
                .parallel_fallback_reason()
                .or_else(|| right.parallel_fallback_reason()),
            Self::Apply(_, outer, inner, _) => outer
                .parallel_fallback_reason()
                .or_else(|| inner.parallel_fallback_reason()),
            Self::HashShuffleJoin(_, left, right, _) => left
                .iter()
                .find_map(|c| c.parallel_fallback_reason())
                .or_else(|| right.iter().find_map(|c| c.parallel_fallback_reason())),
            Self::Source(..) => None,
        }
    }

    /// Record output row count in profile for this operator.
    pub fn record_profile_rows(&self, count: u64) {
        self.base().record_profile_rows(count);
    }

    /// Record peak memory usage in profile for this operator.
    pub fn record_profile_peak_memory(&self, bytes: u64) {
        let Some(rt) = &self.base().runtime else {
            return;
        };
        let Some(entry) = rt.profile().get_entry(&self.profile_key()) else {
            return;
        };
        let prev = entry
            .peak_memory_bytes
            .fetch_max(bytes, std::sync::atomic::Ordering::Relaxed);
        if bytes > prev {
            entry
                .peak_memory_bytes
                .store(bytes, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Record spilled bytes and spill count in profile for this operator.
    pub fn record_profile_spill(&self, spilled_bytes: u64, spill_count: u64) {
        let Some(rt) = &self.base().runtime else {
            return;
        };
        let Some(entry) = rt.profile().get_entry(&self.profile_key()) else {
            return;
        };
        entry
            .spilled_bytes
            .fetch_add(spilled_bytes, std::sync::atomic::Ordering::Relaxed);
        entry
            .spill_count
            .fetch_add(spill_count, std::sync::atomic::Ordering::Relaxed);
    }

    /// Register a resource cleanup callback with the attached runtime.
    pub fn register_resource<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.base().register_resource(f);
    }

    /// Return the base fields.
    pub fn base(&self) -> &OperatorBase {
        match self {
            Self::Source(base, _) => base,
            Self::Unary(base, _, _) => base,
            Self::Join(base, _, _, _) | Self::Set(base, _, _, _) | Self::Apply(base, _, _, _) => {
                base
            }
            Self::Blocking(base, _, _) => base,
            Self::Graph(base, _, _) | Self::RecursiveFragment(base, _, _) => base,
            Self::Sink(base, _, _) => base,
            Self::Ddl(base, _, _) | Self::Fulltext(base, _, _) | Self::Vector(base, _, _) => base,
            Self::Txn(base, _, _) => base,
            Self::Gather(base, _, _) | Self::Exchange(base, _, _) => base,
            Self::HashShuffleJoin(base, _, _, _) => base,
        }
    }

    /// Return the base fields (mutable).
    pub fn base_mut(&mut self) -> &mut OperatorBase {
        match self {
            Self::Source(base, _) => base,
            Self::Unary(base, _, _) => base,
            Self::Join(base, _, _, _) | Self::Set(base, _, _, _) | Self::Apply(base, _, _, _) => {
                base
            }
            Self::Blocking(base, _, _) => base,
            Self::Graph(base, _, _) | Self::RecursiveFragment(base, _, _) => base,
            Self::Sink(base, _, _) => base,
            Self::Ddl(base, _, _) | Self::Fulltext(base, _, _) | Self::Vector(base, _, _) => base,
            Self::Txn(base, _, _) => base,
            Self::Gather(base, _, _) | Self::Exchange(base, _, _) => base,
            Self::HashShuffleJoin(base, _, _, _) => base,
        }
    }

    /// Return mutable references to all child executors.
    pub fn children_mut(&mut self) -> Vec<&mut Self> {
        match self {
            Self::Source(..) => vec![],
            Self::Unary(_, input, _)
            | Self::Blocking(_, input, _)
            | Self::Graph(_, input, _)
            | Self::RecursiveFragment(_, input, _)
            | Self::Sink(_, input, _)
            | Self::Txn(_, input, _) => vec![input.as_mut()],
            Self::Join(_, left, right, _)
            | Self::Set(_, left, right, _)
            | Self::Apply(_, left, right, _) => vec![left.as_mut(), right.as_mut()],
            Self::Ddl(_, input, _) | Self::Fulltext(_, input, _) | Self::Vector(_, input, _) => {
                vec![input.as_mut()]
            }
            Self::Gather(_, children, _) | Self::Exchange(_, children, _) => {
                children.iter_mut().collect()
            }
            Self::HashShuffleJoin(_, left, right, _) => {
                let mut all: Vec<&mut Self> = left.iter_mut().collect();
                all.extend(right.iter_mut());
                all
            }
        }
    }

    /// Access the MemoryTracker for blocking/binary operators.
    pub fn memory_tracker(&self) -> Option<&MemoryTracker> {
        match self {
            Self::Source(..)
            | Self::Unary(..)
            | Self::Graph(..)
            | Self::RecursiveFragment(..)
            | Self::Sink(..)
            | Self::Txn(..)
            | Self::Ddl(..)
            | Self::Fulltext(..)
            | Self::Vector(..)
            | Self::Gather(..)
            | Self::Exchange(..) => None,
            Self::Blocking(_, _, op) => Some(op.memory_tracker()),
            Self::Join(_, _, _, op) => Some(op.memory_tracker()),
            Self::Set(_, _, _, op) => {
                if matches!(&op.kind, SetOperatorKind::UnionAll { .. }) {
                    None
                } else {
                    Some(op.memory_tracker())
                }
            }
            Self::Apply(_, _, _, op) => Some(op.memory_tracker()),
            Self::HashShuffleJoin(_, _, _, op) => Some(op.memory_tracker()),
        }
    }

    /// Whether this operator is in an opened state (can produce chunks).
    pub fn opened(&self) -> bool {
        self.base().lifecycle.is_opened()
    }

    // ── Reset protocol ──

    /// Reset the executor so it re-produces the same logical stream as the
    /// first run (for the same snapshot / correlation frame).
    ///
    /// Lifecycle: `open → (advance)* → reset → (advance)* → … → close`.
    /// The executor instance is reused; no operator structs are rebuilt.
    /// Operators without a native reset degrade to `close + open`
    /// (`reset_used_fallback` is set, surfaced in EXPLAIN).
    pub fn reset(&mut self) -> Result<(), QueryError> {
        let used_fallback = dispatch_reset!(self)?;
        self.base_mut().reset_used_fallback |= used_fallback;
        self.restore_opened_lifecycle();
        Ok(())
    }

    /// Transitional fallback: `close_tree + open` on the whole subtree.
    /// Reuses the existing close/open semantics; never rebuilds operator
    /// structs. Must not become the default path for commonly reset
    /// operators — EXPLAIN marks it via `reset_used_fallback`.
    fn fallback_reset(&mut self) -> Result<bool, QueryError> {
        self.close_tree()?;
        self.mark_tree_new();
        self.open()?;
        Ok(true)
    }

    /// Set every base lifecycle in this tree to `New` (before a re-open).
    fn mark_tree_new(&mut self) {
        self.base_mut().mark_new();
        for child in self.children_mut() {
            child.mark_tree_new();
        }
    }

    /// Mark every base lifecycle in this tree as `Opened` (after a
    /// successful open). Operators no longer touch lifecycle themselves;
    /// the executor is the sole owner of the lifecycle state machine.
    fn mark_tree_opened(&mut self) {
        self.base_mut().lifecycle = OperatorLifecycle::Opened;
        for child in self.children_mut() {
            child.mark_tree_opened();
        }
    }

    /// Restore `Opened` on operators that were opened/exhausted so they can
    /// produce again after a reset.
    fn restore_opened_lifecycle(&mut self) {
        if matches!(
            self.base().lifecycle,
            OperatorLifecycle::Opened | OperatorLifecycle::Exhausted
        ) {
            self.base_mut().lifecycle = OperatorLifecycle::Opened;
        }
        for child in self.children_mut() {
            child.restore_opened_lifecycle();
        }
    }

    /// Inject a correlation frame into the `Argument` source of this
    /// executor tree (the root of a correlated sub-plan). Each executor
    /// instance owns its frame, so parallel partitions and nested
    /// subqueries never interfere.
    pub fn inject_correlation_frame(&mut self, layout: Arc<SlotLayout>, row: Vec<Value>) {
        if let Self::Source(_, op) = self {
            if matches!(&op.kind, SourceOperatorKind::Argument) {
                op.frame = Some((layout, row));
                return;
            }
        }
        for child in self.children_mut() {
            child.inject_correlation_frame(layout.clone(), row.clone());
        }
    }

    // ── Lifecycle dispatch ──

    /// Open the executor.
    pub fn open(&mut self) -> Result<(), QueryError> {
        self.ensure_not_cancelled()?;
        // Pre-register profile entry so the hot path in advance()
        // can find it without write-locking.
        if let Some(rt) = &self.base().runtime {
            let name = self.operator_name();
            let op = OperatorProfile {
                physical_operator_id: self.base().physical_operator_id,
                node_id: self.plan_node_id(),
                partition_id: self.base().partition_id,
                name: name.to_string(),
                ..OperatorProfile::default()
            };
            rt.register_operator(&op);
        }
        // Inject the runtime and execution config (derived from the base
        // fields) into every operator wrapper before any operator code runs.
        let runtime = self.base().runtime.clone();
        let config = OperatorConfig {
            chunk_size: self.base().chunk_size,
            partition_id: self.base().partition_id,
            physical_operator_id: self.base().physical_operator_id,
        };
        self.inject_context(runtime, config);
        let start = Instant::now();
        let result = dispatch_open!(self);
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("open", elapsed);
        if let Err(error) = result {
            if let Err(close_error) = self.close_tree() {
                log::warn!(
                    "Failed to close streaming executor tree after open error: {}",
                    close_error
                );
            }
            return Err(error);
        }
        self.mark_tree_opened();
        Ok(())
    }

    /// Pull the next chunk.
    pub fn advance(&mut self) -> Result<Option<DataChunk>, QueryError> {
        self.ensure_not_cancelled()?;
        // Operators no longer guard on lifecycle themselves; the executor
        // enforces that only opened, non-exhausted executors produce.
        if self.base().lifecycle.is_exhausted() || !self.base().lifecycle.is_opened() {
            return Ok(None);
        }
        let one_shot = matches!(self, Self::Ddl(..) | Self::Fulltext(..) | Self::Vector(..));
        let start = Instant::now();
        let result = dispatch_next!(self);
        let elapsed = start.elapsed().as_micros() as u64;
        if let Ok(Some(ref chunk)) = result {
            self.record_profile_rows(chunk.len() as u64);
        }
        if matches!(&result, Ok(None)) || (one_shot && matches!(&result, Ok(Some(_)))) {
            self.base_mut().lifecycle.mark_exhausted();
        }
        self.record_profile_timing("next", elapsed);
        if result.is_err() {
            self.base_mut().lifecycle.mark_failed();
            if let Err(close_error) = self.close_tree() {
                log::warn!(
                    "Failed to close streaming executor tree after next error: {}",
                    close_error
                );
            }
        }
        result
    }

    /// Stop the executor (signal no more input needed).
    pub fn stop(&mut self) -> Result<(), QueryError> {
        if matches!(
            self.base().lifecycle,
            OperatorLifecycle::Stopped | OperatorLifecycle::Closed
        ) {
            return Ok(());
        }
        let start = Instant::now();
        let result = dispatch_stop!(self);
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("stop", elapsed);
        if result.is_ok() {
            self.base_mut().lifecycle.mark_stopped();
        }
        result
    }

    /// Close the executor (clean up resources).
    pub fn close(&mut self) -> Result<(), QueryError> {
        if matches!(self.base().lifecycle, OperatorLifecycle::Closed) {
            return Ok(());
        }
        // Capture spill metrics before operator close clears state.
        let peak = self.peak_memory_bytes();
        let spilled = self.spilled_size();
        let sc = self.spill_count();
        let start = Instant::now();
        let result = dispatch_close!(self);
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("close", elapsed);
        if peak > 0 {
            self.record_profile_peak_memory(peak);
        }
        if spilled > 0 || sc > 0 {
            self.record_profile_spill(spilled, sc);
        }
        if result.is_ok() {
            self.base_mut().lifecycle.mark_closed();
        }
        result
    }

    /// Close the executor tree in post-order. Individual operators only
    /// release their own state; this is the sole recursive owner.
    pub fn close_tree(&mut self) -> Result<(), QueryError> {
        let mut first_error = None;
        for child in self.children_mut() {
            if let Err(error) = child.close_tree() {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Err(error) = self.close() {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Stop the executor tree. The root is signalled before recursively
    /// stopping children so coordinator operators can cancel worker handles.
    pub fn stop_tree(&mut self) -> Result<(), QueryError> {
        let mut first_error = self.stop().err();
        for child in self.children_mut() {
            if let Err(error) = child.stop_tree() {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Spillable for StreamingExecutor {
    fn spill_to_disk(&mut self) -> Result<(), QueryError> {
        let sm = self
            .base()
            .spill_manager()
            .ok_or_else(|| QueryError::execution("Spill manager not configured"))?;
        match self {
            Self::Blocking(_, _, op) => op.spill_with_manager(&sm),
            Self::Join(_, _, _, op) => op.spill_with_manager(&sm),
            Self::Set(_, _, _, op) => op.spill_with_manager(&sm),
            Self::Apply(_, _, _, _op) => {
                // ApplyOperator doesn't accumulate enough memory;
                // propagate to children instead.
                for child in self.children_mut() {
                    child.spill_to_disk()?;
                }
                Ok(())
            }
            // Other operators (Source, Unary, Graph, etc.) don't accumulate
            // enough memory to warrant spill.  Propagate to children so that
            // deep-nested blocking operators can still be reached.
            Self::Source(..)
            | Self::Unary(..)
            | Self::Graph(..)
            | Self::RecursiveFragment(..)
            | Self::Sink(..)
            | Self::Ddl(..)
            | Self::Fulltext(..)
            | Self::Vector(..)
            | Self::Txn(..)
            | Self::Gather(..)
            | Self::Exchange(..)
            | Self::HashShuffleJoin(..) => {
                for child in self.children_mut() {
                    child.spill_to_disk()?;
                }
                Ok(())
            }
        }
    }

    fn spilled_size(&self) -> u64 {
        match self {
            Self::Blocking(_, _, op) => op.spilled_bytes(),
            Self::Join(_, _, _, op) => op.spilled_bytes(),
            Self::Set(_, _, _, op) => op.spilled_bytes(),
            _ => 0,
        }
    }

    fn spill_count(&self) -> u64 {
        match self {
            Self::Blocking(_, _, op) => op.spill_count(),
            Self::Join(_, _, _, _op) => 0,
            Self::Set(_, _, _, _op) => 0,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockingOperator, ExecutionRuntime, OperatorBase, OperatorProfileKey, SetOperator,
        SortDirection, SourceOperator, StreamingExecutor, UnaryOperator,
    };
    use crate::core::Value;
    use crate::query::executor::streaming::helpers::compare_values;
    use crate::query::executor::streaming::operators::set_operator::SetOperatorKind;
    use crate::query::executor::streaming::operators::source_operator::SourceOperatorKind;
    use crate::query::executor::streaming::operators::unary_operator::UnaryOperatorKind;
    use crate::query::executor::streaming::plan::types::PhysicalOperatorId;
    use crate::query::executor::streaming::slot::SlotLayout;
    use std::sync::Arc;

    fn create_test_buffer() -> Vec<Vec<Value>> {
        (0..100)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::string(format!("vertex_{}", i)),
                    Value::string(format!("label_{}", i % 10)),
                    Value::string(format!("prop_{}", i % 100)),
                    Value::BigInt((i % 1000) as i64),
                ]
            })
            .collect()
    }

    fn scan_executor(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> StreamingExecutor {
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: rows,
                    current_index: 0,
                    col_names,
                },
                Arc::new(SlotLayout::new(vec![])),
            ),
        )
    }

    fn empty_layout() -> Arc<SlotLayout> {
        Arc::new(SlotLayout::new(vec![]))
    }

    #[test]
    fn test_scan_vertices_with_buffer() {
        let buffer = create_test_buffer();
        let mut executor = scan_executor(buffer.clone(), vec![]);

        executor.open().unwrap();
        let chunk = executor.advance().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 100);
        executor.close().unwrap();
    }

    #[test]
    fn test_limit_executor() {
        let buffer = create_test_buffer();
        let scan = Box::new(scan_executor(buffer, vec![]));
        let mut executor = StreamingExecutor::Unary(
            OperatorBase::new(0),
            scan,
            UnaryOperator::new(
                UnaryOperatorKind::Limit {
                    offset: 0,
                    limit: 10,
                    skipped: 0,
                    consumed: 0,
                },
                empty_layout(),
            ),
        );

        executor.open().unwrap();
        let mut total = 0;
        while let Some(mut chunk) = executor.advance().unwrap() {
            // Limit is selection-aware — materialize to count the rows
            // an API consumer would observe (engine does this at the root).
            chunk.materialize_selection();
            total += chunk.len();
        }
        executor.close().unwrap();
        assert_eq!(total, 10);
    }

    #[test]
    fn test_limit_executor_honors_offset() {
        let scan = Box::new(scan_executor(
            (0..6).map(|value| vec![Value::BigInt(value)]).collect(),
            vec!["id".to_string()],
        ));
        let mut executor = StreamingExecutor::Unary(
            OperatorBase::new(0),
            scan,
            UnaryOperator::new(
                UnaryOperatorKind::Limit {
                    offset: 2,
                    limit: 3,
                    skipped: 0,
                    consumed: 0,
                },
                empty_layout(),
            ),
        );

        executor.open().expect("limit should open");
        let mut values = Vec::new();
        while let Some(mut chunk) = executor.advance().expect("limit should advance") {
            // materialize the selection to observe the rows an API
            // consumer would see (engine does this at the root).
            chunk.materialize_selection();
            values.extend(chunk.rows.into_iter().filter_map(|row| match row.first() {
                Some(Value::BigInt(value)) => Some(*value),
                _ => None,
            }));
        }
        executor.close().expect("limit should close");

        assert_eq!(values, vec![2, 3, 4]);
    }

    #[test]
    fn test_dynamic_column_count() {
        let buffer: Vec<Vec<Value>> = vec![
            vec![
                Value::BigInt(1),
                Value::string("a"),
                Value::string("b"),
                Value::string("c"),
                Value::string("d"),
                Value::string("e"),
                Value::string("f"),
                Value::string("g"),
                Value::string("h"),
            ],
            vec![
                Value::BigInt(2),
                Value::string("i"),
                Value::string("j"),
                Value::string("k"),
                Value::string("l"),
                Value::string("m"),
                Value::string("n"),
                Value::string("o"),
                Value::string("p"),
            ],
        ];

        let col_names = (0..9).map(|i| format!("col_{}", i)).collect::<Vec<_>>();
        let mut executor = scan_executor(buffer.clone(), col_names);
        executor.open().unwrap();
        let chunk = executor.advance().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk.num_columns(), 9);
        executor.close().unwrap();
    }

    #[test]
    fn failed_open_closes_children_opened_before_the_failure() {
        let left = StreamingExecutor::Source(
            OperatorBase::new(1),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: vec![vec![Value::BigInt(1)]],
                    current_index: 0,
                    col_names: vec!["id".to_string()],
                },
                empty_layout(),
            ),
        );
        let right = StreamingExecutor::Source(
            OperatorBase::new(2),
            SourceOperator::new(
                SourceOperatorKind::StorageScanVertices {
                    storage: None,
                    space_name: "test".to_string(),
                    limit: None,
                    partition_range: None,
                    col_names: vec![],
                    projected_properties: vec![],
                    predicate: Vec::new(),
                    tag: None,
                    cursor: None,
                },
                empty_layout(),
            ),
        );
        let mut executor = StreamingExecutor::Set(
            OperatorBase::new(3),
            Box::new(left),
            Box::new(right),
            SetOperator::new(
                SetOperatorKind::UnionAll {
                    left_consumed: false,
                },
                empty_layout(),
            ),
        );

        assert!(executor.open().is_err());
        assert!(!executor.opened());
        for child in executor.children_mut() {
            assert!(!child.opened());
        }
    }

    #[test]
    fn test_sort_spill_records_profile_metrics() {
        use crate::query::executor::base::MemoryBudget;
        use crate::query::executor::streaming::operators::spec::BlockingSpec;
        use crate::query::executor::streaming::plan::types::PhysicalOperatorId;
        use crate::query::executor::streaming::slot::SlotLayout;
        use crate::query::executor::streaming::spill::{SpillConfig, SpillManager};
        use std::sync::Arc;

        // Build a runtime with a large memory budget (source chunk
        // reservations must not fail) while the Sort tracker uses a tiny
        // budget so the operator spills immediately.
        let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
        let tracker_budget = MemoryBudget::new(128); // ~3 rows before spill
        let rt = Arc::new(ExecutionRuntime::new(
            super::super::runtime::QueryIdentity {
                query_id: 999,
                session_id: None,
                space_name: None,
            },
            runtime_budget,
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));

        let sm = Arc::new(SpillManager::new(SpillConfig::default(), 999).unwrap());
        rt.set_spill_manager(Some(sm));

        // Build input scan
        let rows: Vec<Vec<Value>> = (0..50)
            .map(|i| vec![Value::BigInt(50 - i as i64)])
            .collect();
        let scan = Box::new(scan_executor(rows, vec!["val".to_string()]));

        let output_layout = Arc::new(SlotLayout::new(vec![]));
        let mut executor = StreamingExecutor::Blocking(
            OperatorBase::new(10)
                .with_runtime(Some(rt.clone()))
                .with_physical_operator_id(PhysicalOperatorId(42))
                .with_output_layout(output_layout.clone()),
            scan,
            BlockingOperator::from_spec(
                &BlockingSpec::Sort {
                    sort_expressions: vec![crate::core::types::expr::Expression::variable(
                        "val".to_string(),
                    )],
                    sort_directions: vec![SortDirection::Ascending],
                },
                &tracker_budget,
                output_layout,
            ),
        );

        executor.open().unwrap();
        while let Some(_chunk) = executor.advance().unwrap() {}
        executor.close().unwrap();

        // Verify profile has spill metrics recorded.
        let prof = rt.profile().flush_to_collector();
        let key = OperatorProfileKey::new(PhysicalOperatorId(42), None);
        let entry = prof.operators.get(&key).expect("profile entry exists");
        assert!(
            entry.spilled_bytes > 0,
            "expected spilled_bytes > 0, got {}",
            entry.spilled_bytes
        );
        assert!(
            entry.spill_count > 0,
            "expected spill_count > 0, got {}",
            entry.spill_count
        );
        assert!(
            entry.peak_memory_bytes > 0,
            "expected peak_memory_bytes > 0, got {}",
            entry.peak_memory_bytes
        );
        assert_eq!(entry.output_rows, 50);
    }

    /// Run a Sort operator over `rows` and return all output rows plus the
    /// runtime (for spill-metric inspection).
    fn run_sort(
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
        spill_budget_bytes: Option<usize>,
    ) -> (Vec<Vec<Value>>, Arc<ExecutionRuntime>) {
        use crate::query::executor::base::MemoryBudget;
        use crate::query::executor::streaming::operators::spec::BlockingSpec;
        use crate::query::executor::streaming::plan::types::PhysicalOperatorId;
        use crate::query::executor::streaming::slot::SlotLayout;
        use crate::query::executor::streaming::spill::{SpillConfig, SpillManager};

        let spill_budget = spill_budget_bytes.unwrap_or(512 * 1024 * 1024);
        let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
        let tracker_budget = MemoryBudget::new(spill_budget);
        let rt = Arc::new(ExecutionRuntime::new(
            super::super::runtime::QueryIdentity {
                query_id: 4243,
                session_id: None,
                space_name: None,
            },
            runtime_budget.clone(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        if spill_budget_bytes.is_some() {
            let sm = Arc::new(SpillManager::new(SpillConfig::default(), 4243).unwrap());
            rt.set_spill_manager(Some(sm));
        }

        let scan = Box::new(scan_executor(rows, col_names.clone()));
        let output_layout = Arc::new(SlotLayout::new(vec![]));
        let mut executor = StreamingExecutor::Blocking(
            OperatorBase::new(10)
                .with_runtime(Some(rt.clone()))
                .with_physical_operator_id(PhysicalOperatorId(44))
                .with_output_layout(output_layout.clone()),
            scan,
            BlockingOperator::from_spec(
                &BlockingSpec::Sort {
                    sort_expressions: vec![crate::core::types::expr::Expression::variable(
                        col_names[0].clone(),
                    )],
                    sort_directions: vec![SortDirection::Ascending],
                },
                &tracker_budget,
                output_layout,
            ),
        );

        executor.open().unwrap();
        let mut result = Vec::new();
        while let Some(chunk) = executor.advance().unwrap() {
            result.extend(chunk.rows);
        }
        executor.close().unwrap();
        (result, rt)
    }

    #[test]
    fn test_sort_multi_run_spill_output_sorted_and_complete() {
        // Many rows with a tiny budget produce multiple spill runs; the merge
        // must reconstruct the fully sorted output (write → read → merge →
        // cleanup closed loop).
        let rows: Vec<Vec<Value>> = (0..500)
            .map(|i| vec![Value::BigInt(499 - i as i64)])
            .collect();
        let col_names = vec!["val".to_string()];

        let (spilled, rt) = run_sort(rows, col_names, Some(64));
        assert_eq!(spilled.len(), 500, "all rows must survive spill");
        for (i, row) in spilled.iter().enumerate() {
            assert_eq!(
                row[0],
                Value::BigInt(i as i64),
                "merged output must be fully sorted at index {}",
                i
            );
        }

        let prof = rt.profile().flush_to_collector();
        let key = OperatorProfileKey::new(PhysicalOperatorId(44), None);
        let entry = prof.operators.get(&key).expect("profile entry exists");
        assert!(
            entry.spill_count >= 2,
            "expected multiple spill runs, got {}",
            entry.spill_count
        );
        assert!(entry.spilled_bytes > 0);

        // Spilled result must be identical to the in-memory baseline.
        let (in_memory, _) = run_sort(
            (0..500)
                .map(|i| vec![Value::BigInt(499 - i as i64)])
                .collect(),
            vec!["val".to_string()],
            None,
        );
        assert_eq!(spilled, in_memory, "spill and in-memory results differ");
    }

    /// Run an Aggregate operator over `rows` and return all result rows.
    ///
    /// When `spill_budget_bytes` is `Some`, a tiny memory budget plus a spill
    /// manager force the accumulator spill path; otherwise a large budget
    /// keeps everything in memory.
    fn run_aggregate(
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
        spill_budget_bytes: Option<usize>,
    ) -> (Vec<Vec<Value>>, Arc<ExecutionRuntime>) {
        use crate::core::types::expr::Expression;
        use crate::core::types::operators::AggregateFunction;
        use crate::query::executor::base::MemoryBudget;
        use crate::query::executor::streaming::operators::spec::BlockingSpec;
        use crate::query::executor::streaming::slot::SlotLayout;
        use crate::query::executor::streaming::spill::{SpillConfig, SpillManager};

        let spill_budget = spill_budget_bytes.unwrap_or(512 * 1024 * 1024);
        let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
        let tracker_budget = MemoryBudget::new(spill_budget);
        let rt = Arc::new(ExecutionRuntime::new(
            super::super::runtime::QueryIdentity {
                query_id: 4242,
                session_id: None,
                space_name: None,
            },
            runtime_budget.clone(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        if spill_budget_bytes.is_some() {
            let sm = Arc::new(SpillManager::new(SpillConfig::default(), 4242).unwrap());
            rt.set_spill_manager(Some(sm));
        }

        let scan = Box::new(scan_executor(rows, col_names));
        let output_layout = Arc::new(SlotLayout::new(vec![]));
        let mut executor = StreamingExecutor::Blocking(
            OperatorBase::new(10)
                .with_runtime(Some(rt.clone()))
                .with_physical_operator_id(PhysicalOperatorId(43))
                .with_output_layout(output_layout.clone()),
            scan,
            BlockingOperator::from_spec(
                &BlockingSpec::Aggregate {
                    group_by_expressions: vec![Expression::variable("g".to_string())],
                    aggregate_functions: vec![
                        (
                            AggregateFunction::Count,
                            vec![Expression::Literal(Value::Int(1))],
                        ),
                        (
                            AggregateFunction::Sum,
                            vec![Expression::variable("v".to_string())],
                        ),
                        (
                            AggregateFunction::Min,
                            vec![Expression::variable("v".to_string())],
                        ),
                        (
                            AggregateFunction::Max,
                            vec![Expression::variable("v".to_string())],
                        ),
                        (
                            AggregateFunction::Collect,
                            vec![Expression::variable("v".to_string())],
                        ),
                    ],
                    output_col_names: vec![],
                },
                &tracker_budget,
                output_layout,
            ),
        );

        executor.open().unwrap();
        let mut result = Vec::new();
        while let Some(chunk) = executor.advance().unwrap() {
            result.extend(chunk.rows);
        }
        executor.close().unwrap();

        result.sort_by(|a, b| compare_values(&a[0], &b[0]));
        (result, rt)
    }

    #[test]
    fn test_aggregate_spill_matches_in_memory() {
        let rows: Vec<Vec<Value>> = (0..2000)
            .map(|i| {
                vec![
                    Value::BigInt((i % 40) as i64),
                    Value::BigInt((i as i64) * 3 - 1000),
                ]
            })
            .collect();
        let col_names = vec!["g".to_string(), "v".to_string()];

        let (spilled, rt) = run_aggregate(rows.clone(), col_names.clone(), Some(4096));
        let (in_memory, _) = run_aggregate(rows, col_names, None);

        // Spilled results must match the in-memory baseline for every group.
        assert_eq!(spilled.len(), in_memory.len());
        for (spilled_row, in_mem_row) in spilled.iter().zip(in_memory.iter()) {
            assert_eq!(spilled_row.len(), in_mem_row.len());
            for (s, m) in spilled_row.iter().zip(in_mem_row.iter()) {
                match (s, m) {
                    (Value::List(a), Value::List(b)) => {
                        assert_eq!(a.values, b.values);
                    }
                    _ => assert_eq!(s, m, "group {:?} value mismatch", spilled_row[0]),
                }
            }
        }

        // The spill path must actually have spilled to disk.
        let prof = rt.profile().flush_to_collector();
        let key = OperatorProfileKey::new(PhysicalOperatorId(43), None);
        let entry = prof.operators.get(&key).expect("profile entry exists");
        assert!(
            entry.spilled_bytes > 0,
            "expected spilled_bytes > 0, got {}",
            entry.spilled_bytes
        );
    }

    /// Run a GroupBy operator over `rows` and return all result rows.
    ///
    /// When `spill_budget_bytes` is `Some`, a tiny memory budget plus a spill
    /// manager force the partition-spill path; otherwise a large budget
    /// keeps everything in memory.
    fn run_groupby(
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
        spill_budget_bytes: Option<usize>,
    ) -> (Vec<Vec<Value>>, Arc<ExecutionRuntime>) {
        use crate::query::executor::base::MemoryBudget;
        use crate::query::executor::streaming::operators::spec::BlockingSpec;
        use crate::query::executor::streaming::plan::types::PhysicalOperatorId;
        use crate::query::executor::streaming::slot::SlotLayout;
        use crate::query::executor::streaming::spill::{SpillConfig, SpillManager};

        let spill_budget = spill_budget_bytes.unwrap_or(512 * 1024 * 1024);
        let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
        let tracker_budget = MemoryBudget::new(spill_budget);
        let rt = Arc::new(ExecutionRuntime::new(
            super::super::runtime::QueryIdentity {
                query_id: 4244,
                session_id: None,
                space_name: None,
            },
            runtime_budget.clone(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        if spill_budget_bytes.is_some() {
            let sm = Arc::new(SpillManager::new(SpillConfig::default(), 4244).unwrap());
            rt.set_spill_manager(Some(sm));
        }

        let scan = Box::new(scan_executor(rows, col_names.clone()));
        let output_layout = Arc::new(SlotLayout::new(vec![]));
        let mut executor = StreamingExecutor::Blocking(
            OperatorBase::new(10)
                .with_runtime(Some(rt.clone()))
                .with_physical_operator_id(PhysicalOperatorId(45))
                .with_output_layout(output_layout.clone()),
            scan,
            BlockingOperator::from_spec(
                &BlockingSpec::GroupBy {
                    group_by_expressions: vec![crate::core::types::expr::Expression::variable(
                        col_names[0].clone(),
                    )],
                },
                &tracker_budget,
                output_layout,
            ),
        );

        executor.open().unwrap();
        let mut result = Vec::new();
        while let Some(chunk) = executor.advance().unwrap() {
            result.extend(chunk.rows);
        }
        executor.close().unwrap();

        // GroupBy is a grouping (not sorting) operator; normalize order for
        // comparison by sorting on the full row content.
        result.sort_by(|a, b| {
            for (x, y) in a.iter().zip(b.iter()) {
                let c = compare_values(x, y);
                if c != std::cmp::Ordering::Equal {
                    return c;
                }
            }
            std::cmp::Ordering::Equal
        });
        (result, rt)
    }

    #[test]
    fn test_groupby_spill_matches_in_memory() {
        let rows: Vec<Vec<Value>> = (0..2000)
            .map(|i| {
                vec![
                    Value::BigInt((i % 40) as i64),
                    Value::BigInt((i as i64) * 3 - 1000),
                ]
            })
            .collect();
        let col_names = vec!["g".to_string(), "v".to_string()];

        let (spilled, rt) = run_groupby(rows.clone(), col_names.clone(), Some(65536));
        let (in_memory, _) = run_groupby(rows, col_names, None);

        // Grouped output must be identical to the in-memory baseline.
        assert_eq!(spilled, in_memory, "spill and in-memory results differ");

        // The spill path must actually have spilled to disk.
        let prof = rt.profile().flush_to_collector();
        let key = OperatorProfileKey::new(PhysicalOperatorId(45), None);
        let entry = prof.operators.get(&key).expect("profile entry exists");
        assert!(
            entry.spilled_bytes > 0,
            "expected spilled_bytes > 0, got {}",
            entry.spilled_bytes
        );
        assert!(
            entry.spill_count > 0,
            "expected spill_count > 0, got {}",
            entry.spill_count
        );
    }

    /// Run a WindowFunction operator over `rows` and return all result rows.
    ///
    /// When `spill_budget_bytes` is `Some`, a tiny memory budget plus a spill
    /// manager force the partition-spill path; otherwise a large budget
    /// keeps everything in memory.
    fn run_window(
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
        spill_budget_bytes: Option<usize>,
    ) -> (Vec<Vec<Value>>, Arc<ExecutionRuntime>) {
        use crate::core::types::expr::Expression;
        use crate::query::executor::base::MemoryBudget;
        use crate::query::executor::streaming::operators::spec::BlockingSpec;
        use crate::query::executor::streaming::plan::types::PhysicalOperatorId;
        use crate::query::executor::streaming::slot::SlotLayout;
        use crate::query::executor::streaming::spill::{SpillConfig, SpillManager};

        let spill_budget = spill_budget_bytes.unwrap_or(512 * 1024 * 1024);
        let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
        let tracker_budget = MemoryBudget::new(spill_budget);
        let rt = Arc::new(ExecutionRuntime::new(
            super::super::runtime::QueryIdentity {
                query_id: 4245,
                session_id: None,
                space_name: None,
            },
            runtime_budget.clone(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        if spill_budget_bytes.is_some() {
            let sm = Arc::new(SpillManager::new(SpillConfig::default(), 4245).unwrap());
            rt.set_spill_manager(Some(sm));
        }

        let scan = Box::new(scan_executor(rows, col_names.clone()));
        let output_layout = Arc::new(SlotLayout::new(vec![]));
        let mut executor = StreamingExecutor::Blocking(
            OperatorBase::new(10)
                .with_runtime(Some(rt.clone()))
                .with_physical_operator_id(PhysicalOperatorId(46))
                .with_output_layout(output_layout.clone()),
            scan,
            BlockingOperator::from_spec(
                &BlockingSpec::WindowFunction {
                    window_exprs: vec![Expression::WindowFunction {
                        name: "row_number".to_string(),
                        args: vec![],
                        over_partition_by: vec![Expression::variable(col_names[0].clone())],
                        over_order_by: vec![Expression::variable(col_names[1].clone())],
                        over_order_desc: vec![false],
                    }],
                    partition_by_exprs: vec![Expression::variable(col_names[0].clone())],
                    order_by_exprs: vec![Expression::variable(col_names[1].clone())],
                    order_by_directions: vec![SortDirection::Ascending],
                },
                &tracker_budget,
                output_layout,
            ),
        );

        executor.open().unwrap();
        let mut result = Vec::new();
        while let Some(chunk) = executor.advance().unwrap() {
            result.extend(chunk.rows);
        }
        executor.close().unwrap();

        // Partitions are emitted in different orders on the two paths; sort
        // rows by content so the comparison is order-independent.
        result.sort_by(|a, b| {
            for (x, y) in a.iter().zip(b.iter()) {
                let c = compare_values(x, y);
                if c != std::cmp::Ordering::Equal {
                    return c;
                }
            }
            std::cmp::Ordering::Equal
        });
        (result, rt)
    }

    #[test]
    fn test_window_spill_matches_in_memory() {
        let rows: Vec<Vec<Value>> = (0..2000)
            .map(|i| {
                vec![
                    Value::BigInt((i % 40) as i64),
                    Value::BigInt(((i * 7) % 1000) as i64),
                ]
            })
            .collect();
        let col_names = vec!["p".to_string(), "v".to_string()];

        let (spilled, rt) = run_window(rows.clone(), col_names.clone(), Some(65536));
        let (in_memory, _) = run_window(rows, col_names, None);

        // Window output must be identical to the in-memory baseline.
        assert_eq!(spilled, in_memory, "spill and in-memory results differ");
        assert!(!spilled.is_empty(), "window produced no output rows");

        // The spill path must actually have spilled to disk.
        let prof = rt.profile().flush_to_collector();
        let key = OperatorProfileKey::new(PhysicalOperatorId(46), None);
        let entry = prof.operators.get(&key).expect("profile entry exists");
        assert!(
            entry.spilled_bytes > 0,
            "expected spilled_bytes > 0, got {}",
            entry.spilled_bytes
        );
        assert!(
            entry.spill_count > 0,
            "expected spill_count > 0, got {}",
            entry.spill_count
        );
    }

    // ── Reset protocol ──

    fn pull_all(executor: &mut StreamingExecutor) -> Vec<Vec<Value>> {
        let mut rows = Vec::new();
        while let Some(mut chunk) = executor.advance().expect("advance should succeed") {
            chunk.materialize_selection_by("reset-test");
            rows.extend(chunk.rows);
        }
        rows
    }

    #[test]
    fn stateless_filter_reset_repulls_identical_output() {
        use crate::core::types::expr::Expression;
        use crate::core::types::operators::BinaryOperator;

        let scan = Box::new(scan_executor(
            (1..=6).map(|v| vec![Value::BigInt(v)]).collect(),
            vec!["v".to_string()],
        ));
        let predicate = Expression::binary(
            Expression::variable("v"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(3)),
        );
        let mut executor = StreamingExecutor::Unary(
            OperatorBase::new(0),
            scan,
            UnaryOperator::new(
                UnaryOperatorKind::Filter {
                    predicate,
                    state: Default::default(),
                },
                empty_layout(),
            ),
        );

        executor.open().expect("open should succeed");
        let first = pull_all(&mut executor);
        assert_eq!(
            first,
            vec![
                vec![Value::BigInt(4)],
                vec![Value::BigInt(5)],
                vec![Value::BigInt(6)]
            ]
        );

        executor.reset().expect("reset should succeed");
        let second = pull_all(&mut executor);
        assert_eq!(second, first, "stateless filter reset re-produces output");
        executor.close().expect("close should succeed");
    }

    #[test]
    fn buffered_unary_reset_clears_counters_and_seen_rows() {
        let dedup_scan = Box::new(scan_executor(
            vec![
                vec![Value::BigInt(1)],
                vec![Value::BigInt(1)],
                vec![Value::BigInt(2)],
            ],
            vec!["v".to_string()],
        ));
        let mut dedup = StreamingExecutor::Unary(
            OperatorBase::new(0),
            dedup_scan,
            UnaryOperator::new(
                UnaryOperatorKind::Dedup {
                    seen_rows: std::collections::HashSet::new(),
                },
                empty_layout(),
            ),
        );
        dedup.open().expect("open should succeed");
        let first = pull_all(&mut dedup);
        assert_eq!(first, vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]]);
        dedup.reset().expect("dedup reset should succeed");
        let second = pull_all(&mut dedup);
        assert_eq!(second, first, "dedup seen_rows must be cleared by reset");
        dedup.close().expect("close should succeed");

        let limit_scan = Box::new(scan_executor(
            (0..10).map(|v| vec![Value::BigInt(v)]).collect(),
            vec!["v".to_string()],
        ));
        let mut limit = StreamingExecutor::Unary(
            OperatorBase::new(0),
            limit_scan,
            UnaryOperator::new(
                UnaryOperatorKind::Limit {
                    offset: 0,
                    limit: 3,
                    skipped: 0,
                    consumed: 0,
                },
                empty_layout(),
            ),
        );
        limit.open().expect("open should succeed");
        let first = pull_all(&mut limit);
        assert_eq!(first.len(), 3, "limit applies on the first run");
        limit.reset().expect("limit reset should succeed");
        let second = pull_all(&mut limit);
        assert_eq!(second, first, "limit counters must be reset");
        limit.close().expect("close should succeed");
    }

    #[test]
    fn blocking_sort_reset_falls_back_to_close_open_and_marks_flag() {
        use crate::core::types::expr::Expression;
        use crate::query::executor::streaming::operators::spec::BlockingSpec;
        use crate::query::executor::streaming::slot::SlotLayout;

        let rows: Vec<Vec<Value>> = (0..6).map(|v| vec![Value::BigInt(5 - v)]).collect();
        let scan = Box::new(scan_executor(rows, vec!["v".to_string()]));
        let output_layout = Arc::new(SlotLayout::new(vec![]));
        let mut executor = StreamingExecutor::Blocking(
            OperatorBase::new(10).with_output_layout(output_layout.clone()),
            scan,
            BlockingOperator::from_spec(
                &BlockingSpec::Sort {
                    sort_expressions: vec![Expression::variable("v".to_string())],
                    sort_directions: vec![SortDirection::Ascending],
                },
                &crate::query::executor::base::MemoryBudget::default_budget(),
                output_layout,
            ),
        );

        executor.open().expect("open should succeed");
        let first = pull_all(&mut executor);
        assert_eq!(first.len(), 6);
        executor.reset().expect("reset should succeed");
        assert!(
            executor.base().reset_used_fallback,
            "Blocking has no native reset yet; fallback must be flagged"
        );
        let second = pull_all(&mut executor);
        assert_eq!(
            second, first,
            "fallback reset re-produces the sorted stream"
        );
        executor.close().expect("close should succeed");
    }

    #[test]
    fn correlation_frames_are_isolated_per_executor_instance() {
        use crate::query::executor::streaming::slot::SlotLayout;

        let layout = Arc::new(SlotLayout::from_names(&["id".to_string()]));
        let mut first = StreamingExecutor::Source(
            OperatorBase::new(0).with_output_layout(layout.clone()),
            SourceOperator::new(SourceOperatorKind::Argument, layout.clone()),
        );
        let mut second = StreamingExecutor::Source(
            OperatorBase::new(1).with_output_layout(layout.clone()),
            SourceOperator::new(SourceOperatorKind::Argument, layout.clone()),
        );
        first.open().expect("open should succeed");
        second.open().expect("open should succeed");

        first.inject_correlation_frame(layout.clone(), vec![Value::BigInt(10)]);
        second.inject_correlation_frame(layout.clone(), vec![Value::BigInt(20)]);

        let first_chunk = first.advance().expect("pull").expect("first frame row");
        let second_chunk = second.advance().expect("pull").expect("second frame row");
        assert_eq!(first_chunk.rows, vec![vec![Value::BigInt(10)]]);
        assert_eq!(
            second_chunk.rows,
            vec![vec![Value::BigInt(20)]],
            "frames must be private to each executor instance"
        );

        first.reset().expect("reset should succeed");
        second.reset().expect("reset should succeed");
        first.inject_correlation_frame(layout.clone(), vec![Value::BigInt(30)]);
        let again = first.advance().expect("pull").expect("third frame row");
        assert_eq!(again.rows, vec![vec![Value::BigInt(30)]]);
    }
}
