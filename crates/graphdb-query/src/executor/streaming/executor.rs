//! StreamingExecutor: Thin dispatch layer over domain-specific operator enums

use std::sync::Arc;
use std::time::Instant;

use super::chunk::DataChunk;
use super::runtime::{ExecutionRuntime, OperatorProfile, OperatorProfileKey};
use super::slot::SlotLayout;
use crate::executor::base::{MemoryTracker, Spillable};
use graphdb_core::error::QueryError;
use graphdb_core::Value;

pub use super::context::ValueRowContext;
pub use super::helpers::{comparison, conversion};
pub use super::operators::base::OperatorBase;
use super::operators::base::OperatorLifecycle;
use super::operators::source_operator::OperatorConfig;

use super::operators::apply_operator::ApplyOperator;
use super::operators::blocking::BlockingOperator;
use super::operators::blocking::BlockingOperatorKind;
use super::operators::ddl_operator::DdlOperator;
use super::operators::exchange_operator::ExchangeOperator;
use super::operators::fulltext_operator::FulltextOperator;
use super::operators::gather_operator::GatherOperator;
use super::operators::graph_operator::GraphOperator;
use super::operators::graph_operator::GraphOperatorKind;
use super::operators::join_operator::JoinOperator;
use super::operators::recursive_fragment_operator::RecursiveFragmentOperator;
use super::operators::set_operator::SetOperator;
use super::operators::set_operator::SetOperatorKind;
use super::operators::shuffle_join_operator::HashShuffleJoinOperator;
use super::operators::sink_operator::SinkOperator;
use super::operators::source_operator::SourceOperator;
use super::operators::source_operator::SourceOperatorKind;
use super::operators::txn_operator::TxnOperator;
use super::operators::unary_operator::UnaryOperator;
use super::operators::unary_operator::UnaryOperatorKind;
use super::operators::vector_operator::VectorOperator;
use super::operators::wco_operator::WcoIntersectOperator;

mod operator_name;
#[cfg(test)]
mod tests;

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

/// StreamingExecutor: 17-variant dispatch enum over domain-specific operators.
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
    /// N-way worst-case-optimal intersect: one probe input plus N build
    /// inputs sharing a single intersect variable.
    Wco(
        OperatorBase,
        Box<StreamingExecutor>,
        Vec<StreamingExecutor>,
        WcoIntersectOperator,
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
            Self::Wco(_, probe, builds, op) => op.open(probe, builds),
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
            Self::Wco(_, probe, builds, op) => op.next(probe, builds),
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
            Self::Wco(_, probe, builds, op) => op.reset(probe, builds),
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
            Self::Wco(_, _, _, op) => op.stop(),
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
            Self::Wco(_, _, _, op) => op.close(),
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
            Self::Wco(_, _, _, op) => op.inject_context(runtime_ref, config),
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
                    // Morsel parallel: Sort/Agg are partition-local
                    // (local sort + merge, partial/final aggregate). Hash
                    // aggregation uses `PartialAggregate` (partition-local) +
                    // `FinalAggregate` (global gather) two-phase; `Sort` is
                    // local + `Exchange::MergeSort`. `TopN` is bounded local
                    // sort, `Distinct` and `GroupBy` are hash-partitioned.
                    BlockingOperatorKind::PartialAggregate { .. }
                        | BlockingOperatorKind::Sort { .. }
                        | BlockingOperatorKind::Distinct { .. }
                        | BlockingOperatorKind::TopN { .. }
                        | BlockingOperatorKind::GroupBy { .. }
                        | BlockingOperatorKind::Aggregate { .. }
                ) && input.is_partition_local()
            }
            Self::Graph(_, input, op) => {
                // Single-hop Expand/BiExpand are morsel-parallel
                // (partitioned by anchor vertex-id range, batched neighbor
                // scan via `neighbor_dst_ids_batch`).
                matches!(
                    &op.kind,
                    GraphOperatorKind::Expand { .. }
                        | GraphOperatorKind::BiExpand { .. }
                        | GraphOperatorKind::ExpandAll { .. }
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
        operator_name::operator_name(self)
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
            Self::Wco(_, probe, builds, _) => probe
                .parallel_fallback_reason()
                .or_else(|| builds.iter().find_map(|c| c.parallel_fallback_reason())),
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
            Self::Wco(base, _, _, _) => base,
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
            Self::Wco(base, _, _, _) => base,
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
            Self::Wco(_, probe, builds, _) => {
                let mut all = vec![probe.as_mut()];
                all.extend(builds.iter_mut());
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
            Self::Wco(_, _, _, op) => Some(op.memory_tracker()),
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
            // Logical rows (visible * multiplicity), not physical length.
            self.record_profile_rows(chunk.logical_len());
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
            Self::Wco(_, _, _, op) => op.spill_with_manager(&sm),
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
            Self::Wco(_, _, _, op) => op.spilled_bytes(),
            _ => 0,
        }
    }

    fn spill_count(&self) -> u64 {
        match self {
            Self::Blocking(_, _, op) => op.spill_count(),
            Self::Join(_, _, _, _op) => 0,
            Self::Set(_, _, _, _op) => 0,
            Self::Wco(_, _, _, _op) => 0,
            _ => 0,
        }
    }
}
