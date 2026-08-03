//! StreamingExecutor: Thin dispatch layer over domain-specific operator enums

use std::sync::Arc;
use std::time::Instant;

use super::chunk::DataChunk;
use super::runtime::{ExecutionRuntime, OperatorProfile, OperatorProfileKey};
use crate::core::error::QueryError;
use crate::query::executor::base::{MemoryTracker, Spillable};

pub use super::context::ValueRowContext;
pub use super::helpers::{comparison, conversion};
pub use super::operators::base::OperatorBase;
use super::operators::base::OperatorLifecycle;
use super::operators::state::ExchangeState;

use super::operators::apply_operator::ApplyOperator;
use super::operators::blocking::BlockingOperator;
use super::operators::ddl_operator::DdlOperator;
use super::operators::exchange_operator::ExchangeOperator;
use super::operators::fulltext_operator::FulltextOperator;
use super::operators::gather_operator::GatherOperator;
use super::operators::graph_operator::GraphOperator;
use super::operators::join_operator::JoinOperator;
use super::operators::recursive_fragment_operator::RecursiveFragmentOperator;
use super::operators::set_operator::SetOperator;
use super::operators::shuffle_join_operator::HashShuffleJoinOperator;
use super::operators::sink_operator::SinkOperator;
use super::operators::source_operator::SourceOperator;
use super::operators::txn_operator::TxnOperator;
use super::operators::unary_operator::UnaryOperator;
use super::operators::vector_operator::VectorOperator;

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

/// StreamingExecutor: 15-variant dispatch enum over domain-specific operators.
///
/// Each variant holds an OperatorBase (shared fields), zero or more child
/// executors, and a domain-specific operator enum that implements the
/// per-operator lifecycle logic.
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

/// Dispatch to the correct operator's lifecycle method.
macro_rules! dispatch {
    ($self:expr, $method:ident) => {
        match $self {
            Self::Source(base, op) => op.$method(base),
            Self::Unary(base, child, op) => op.$method(base, child),
            Self::Join(base, left, right, op) => op.$method(base, left, right),
            Self::Set(base, left, right, op) => op.$method(base, left, right),
            Self::Apply(base, left, right, op) => op.$method(base, left, right),
            Self::Blocking(base, child, op) => op.$method(base, child),
            Self::Graph(base, child, op) => op.$method(base, child),
            Self::RecursiveFragment(base, child, op) => op.$method(base, child),
            Self::Sink(base, child, op) => op.$method(base, child),
            Self::Ddl(base, child, op) => op.$method(base, child),
            Self::Fulltext(base, child, op) => op.$method(base, child),
            Self::Vector(base, child, op) => op.$method(base, child),
            Self::Txn(base, child, op) => op.$method(base, child),
            Self::Gather(base, children, op) => op.$method(base, children),
            Self::Exchange(base, children, op) => op.$method(base, children),
            Self::HashShuffleJoin(base, left, right, op) => op.$method(base, left, right),
        }
    };
}

impl StreamingExecutor {
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
            Self::Source(
                _,
                SourceOperator::ScanVertices { .. }
                | SourceOperator::StorageScanVertices { .. }
                | SourceOperator::ScanEdges { .. }
                | SourceOperator::StorageScanEdges { .. },
            ) => true,
            Self::Unary(_, input, op) => {
                matches!(
                    op,
                    UnaryOperator::Filter { .. }
                        | UnaryOperator::Project { .. }
                        | UnaryOperator::Assign { .. }
                        | UnaryOperator::Remove { .. }
                        | UnaryOperator::Unwind { .. }
                        | UnaryOperator::AppendVertices { .. }
                ) && input.is_partition_local()
            }
            Self::Blocking(_, input, op) => {
                matches!(
                    op,
                    BlockingOperator::PartialAggregate { .. }
                        | BlockingOperator::Distinct { .. }
                        | BlockingOperator::TopN { .. }
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
            Source(_, op) => match op {
                SourceOperator::ScanVertices { .. }
                | SourceOperator::StorageScanVertices { .. }
                | SourceOperator::StandaloneValues { .. } => "ScanVertices",
                SourceOperator::ScanEdges { .. } | SourceOperator::StorageScanEdges { .. } => {
                    "ScanEdges"
                }
                SourceOperator::GetVertices { .. } => "GetVertices",
                SourceOperator::GetEdges { .. } => "GetEdges",
                SourceOperator::GetNeighbors { .. } => "GetNeighbors",
                SourceOperator::IndexScan { .. } => "IndexScan",
                SourceOperator::Argument => "Argument",
                SourceOperator::GetProp { .. } => "GetProp",
                SourceOperator::Start => "Start",
            },
            Unary(_, _, op) => match op {
                UnaryOperator::Filter { .. } => "Filter",
                UnaryOperator::Project { .. } => "Project",
                UnaryOperator::Limit { .. } => "Limit",
                UnaryOperator::Dedup { .. } => "Dedup",
                UnaryOperator::Assign { .. } => "Assign",
                UnaryOperator::Remove { .. } => "Remove",
                UnaryOperator::Unwind { .. } => "Unwind",
                UnaryOperator::AppendVertices { .. } => "AppendVertices",
                UnaryOperator::Sample { .. } => "Sample",
            },
            Txn(_, _, op) => match op {
                TxnOperator::BeginTransaction { .. } => "BeginTransaction",
                TxnOperator::Commit { .. } => "Commit",
                TxnOperator::Rollback { .. } => "Rollback",
            },
            Join(_, _, _, op) => match op {
                JoinOperator::HashJoin { .. } => "HashJoin",
                JoinOperator::HashLeftJoin { .. } => "HashLeftJoin",
                JoinOperator::NestedLoopJoin { .. } => "NestedLoopJoin",
                JoinOperator::InnerJoin { .. } => "InnerJoin",
                JoinOperator::LeftJoin { .. } => "LeftJoin",
                JoinOperator::RightJoin { .. } => "RightJoin",
                JoinOperator::FullOuterJoin { .. } => "FullOuterJoin",
                JoinOperator::CrossJoin { .. } => "CrossJoin",
                JoinOperator::SemiJoin { .. } => "SemiJoin",
            },
            Set(_, _, _, op) => match op {
                SetOperator::Union { .. } => "Union",
                SetOperator::UnionAll { .. } => "UnionAll",
                SetOperator::Intersect { .. } => "Intersect",
                SetOperator::Except { .. } => "Except",
                SetOperator::Minus { .. } => "Minus",
            },
            Apply(_, _, _, op) => match op {
                ApplyOperator::Apply { .. } => "Apply",
                ApplyOperator::PatternApply { .. } => "PatternApply",
                ApplyOperator::RollUpApply { .. } => "RollUpApply",
            },
            Blocking(_, _, op) => match op {
                BlockingOperator::Sort { .. } => "Sort",
                BlockingOperator::Aggregate { .. } => "Aggregate",
                BlockingOperator::GroupBy { .. } => "GroupBy",
                BlockingOperator::WindowFunction { .. } => "WindowFunction",
                BlockingOperator::Window { .. } => "Window",
                BlockingOperator::TopN { .. } => "TopN",
                BlockingOperator::Distinct { .. } => "Distinct",
                BlockingOperator::Materialize { .. } => "Materialize",
                BlockingOperator::DataCollect { .. } => "DataCollect",
                BlockingOperator::RollUpApply { .. } => "RollUpApply",
                BlockingOperator::PartialAggregate { .. } => "PartialAggregate",
                BlockingOperator::FinalAggregate { .. } => "FinalAggregate",
            },
            Graph(_, _, op) => match op {
                GraphOperator::Expand { .. } => "Expand",
                GraphOperator::ExpandAll { .. } => "ExpandAll",
                GraphOperator::Traverse { .. } => "Traverse",
                GraphOperator::TraverseAll { .. } => "TraverseAll",
                GraphOperator::BiExpand { .. } => "BiExpand",
                GraphOperator::BiTraverse { .. } => "BiTraverse",
                GraphOperator::Subgraph { .. } => "Subgraph",
            },
            RecursiveFragment(_, _, op) => match op {
                RecursiveFragmentOperator::ShortestPath { .. } => "RecursiveShortestPath",
                RecursiveFragmentOperator::MultiShortestPath { .. } => "RecursiveMultiShortestPath",
                RecursiveFragmentOperator::BFSShortest { .. } => "RecursiveBFSShortest",
                RecursiveFragmentOperator::AllPaths { .. } => "RecursiveAllPaths",
            },
            Sink(_, _, op) => match op {
                SinkOperator::InsertVertices { .. } => "InsertVertices",
                SinkOperator::InsertEdges { .. } => "InsertEdges",
                SinkOperator::UpdateVertices { .. } => "UpdateVertices",
                SinkOperator::UpdateEdges { .. } => "UpdateEdges",
                SinkOperator::DeleteVertices { .. } => "DeleteVertices",
                SinkOperator::DeleteEdges { .. } => "DeleteEdges",
                SinkOperator::PipeDeleteVertices { .. } => "PipeDeleteVertices",
                SinkOperator::PipeDeleteEdges { .. } => "PipeDeleteEdges",
                SinkOperator::DeleteTags { .. } => "DeleteTags",
            },
            Ddl(_, _, op) => match op {
                DdlOperator::SpaceManage { .. } => "SpaceManage",
                DdlOperator::TagManage { .. } => "TagManage",
                DdlOperator::EdgeManage { .. } => "EdgeManage",
                DdlOperator::IndexManage { .. } => "IndexManage",
                DdlOperator::DeleteIndex { .. } => "DeleteIndex",
                DdlOperator::UserManage { .. } => "UserManage",
                DdlOperator::ShowStats { .. } => "ShowStats",
                DdlOperator::ShowConfigs { .. } => "ShowConfigs",
                DdlOperator::ShowQueries { .. } => "ShowQueries",
                DdlOperator::ShowSessions { .. } => "ShowSessions",
                DdlOperator::Analyze { .. } => "Analyze",
                DdlOperator::Migrate { .. } => "Migrate",
            },
            Fulltext(_, _, op) => match op {
                FulltextOperator::FulltextManage { .. } => "FulltextManage",
                FulltextOperator::FulltextSearch { .. } => "FulltextSearch",
                FulltextOperator::FulltextLookup { .. } => "FulltextLookup",
                FulltextOperator::MatchFulltext { .. } => "MatchFulltext",
            },
            Vector(_, _, op) => match op {
                VectorOperator::VectorManage { .. } => "VectorManage",
                VectorOperator::VectorSearch { .. } => "VectorSearch",
                VectorOperator::VectorLookup { .. } => "VectorLookup",
                VectorOperator::VectorMatch { .. } => "VectorMatch",
            },
            Gather(_, _, op) => match op {
                GatherOperator::Concatenate { .. } => "Gather(Concatenate)",
                GatherOperator::MergeSort { .. } => "Gather(MergeSort)",
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

    /// Return the P8 parallel fallback reason from any operator in this
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
            | Self::Apply(..)
            | Self::Ddl(..)
            | Self::Fulltext(..)
            | Self::Vector(..)
            | Self::Gather(..)
            | Self::Exchange(..) => None,
            Self::Blocking(_, _, op) => Some(op.memory_tracker()),
            Self::Join(_, _, _, op) => Some(op.memory_tracker()),
            Self::Set(_, _, _, SetOperator::UnionAll { .. }) => None,
            Self::Set(_, _, _, op) => Some(op.memory_tracker()),
            Self::HashShuffleJoin(_, _, _, op) => Some(op.memory_tracker()),
        }
    }

    /// Whether this operator is in an opened state (can produce chunks).
    pub fn opened(&self) -> bool {
        self.base().lifecycle.is_opened()
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
        let start = Instant::now();
        let result = dispatch!(self, open);
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
        Ok(())
    }

    /// Pull the next chunk.
    pub fn advance(&mut self) -> Result<Option<DataChunk>, QueryError> {
        self.ensure_not_cancelled()?;
        if self.base().lifecycle.is_exhausted() {
            return Ok(None);
        }
        let one_shot = matches!(self, Self::Ddl(..) | Self::Fulltext(..) | Self::Vector(..));
        let start = Instant::now();
        let result = dispatch!(self, next);
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
        let result = dispatch!(self, stop);
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
        let result = dispatch!(self, close);
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
    use crate::query::executor::streaming::plan::types::PhysicalOperatorId;
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
        use super::super::operators::source_operator::SourceOperator;
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                buffer: rows,
                current_index: 0,
                col_names,
            },
        )
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
            UnaryOperator::Limit {
                offset: 0,
                limit: 10,
                skipped: 0,
                consumed: 0,
            },
        );

        executor.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = executor.advance().unwrap() {
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
            UnaryOperator::Limit {
                offset: 2,
                limit: 3,
                skipped: 0,
                consumed: 0,
            },
        );

        executor.open().expect("limit should open");
        let mut values = Vec::new();
        while let Some(chunk) = executor.advance().expect("limit should advance") {
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
            SourceOperator::ScanVertices {
                buffer: vec![vec![Value::BigInt(1)]],
                current_index: 0,
                col_names: vec!["id".to_string()],
            },
        );
        let right = StreamingExecutor::Source(
            OperatorBase::new(2),
            SourceOperator::StorageScanVertices {
                storage: None,
                space_name: "test".to_string(),
                limit: None,
                partition_range: None,
                col_names: vec![],
                projected_properties: vec![],
                predicate: Vec::new(),
                cursor: None,
            },
        );
        let mut executor = StreamingExecutor::Set(
            OperatorBase::new(3),
            Box::new(left),
            Box::new(right),
            SetOperator::UnionAll {
                left_consumed: false,
            },
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

        // Build a runtime with a tiny memory budget so Sort spills immediately.
        let budget = MemoryBudget::new(128); // ~3 rows before spill
        let rt = Arc::new(ExecutionRuntime::new(
            super::super::runtime::QueryIdentity {
                query_id: 999,
                session_id: None,
                space_name: None,
            },
            budget.clone(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "qdrant")]
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
                .with_output_layout(output_layout),
            scan,
            BlockingOperator::from_spec(
                &BlockingSpec::Sort {
                    sort_expressions: vec![crate::core::types::expr::Expression::variable(
                        "val".to_string(),
                    )],
                    sort_directions: vec![SortDirection::Ascending],
                },
                &budget,
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

        let budget = MemoryBudget::new(spill_budget_bytes.unwrap_or(512 * 1024 * 1024));
        let rt = Arc::new(ExecutionRuntime::new(
            super::super::runtime::QueryIdentity {
                query_id: 4242,
                session_id: None,
                space_name: None,
            },
            budget.clone(),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "qdrant")]
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
                .with_output_layout(output_layout),
            scan,
            BlockingOperator::from_spec(
                &BlockingSpec::Aggregate {
                    group_by_expressions: vec![Expression::variable("g".to_string())],
                    aggregate_functions: vec![
                        (
                            AggregateFunction::Count(None),
                            Expression::Literal(Value::Int(1)),
                        ),
                        (
                            AggregateFunction::Sum("v".to_string()),
                            Expression::variable("v".to_string()),
                        ),
                        (
                            AggregateFunction::Min("v".to_string()),
                            Expression::variable("v".to_string()),
                        ),
                        (
                            AggregateFunction::Max("v".to_string()),
                            Expression::variable("v".to_string()),
                        ),
                        (
                            AggregateFunction::Collect("v".to_string()),
                            Expression::variable("v".to_string()),
                        ),
                    ],
                    output_col_names: vec![],
                },
                &budget,
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
}
