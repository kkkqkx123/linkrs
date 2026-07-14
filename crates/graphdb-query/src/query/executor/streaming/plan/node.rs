//! PhysicalNode: Immutable physical plan tree (plan + config, no mutable state).
//!
//! A `PhysicalNode` tree can be cached, cloned, EXPLAINed, and repeatedly
//! materialized into [`StreamingExecutor`] trees — each with fresh
//! [`OperatorState`] — without sharing any mutable state across executions.
//!
//! # Construction flow
//!
//! ```text
//! PlanNodeEnum
//!   -> PhysicalNode (via builder, one-time cost, cachable)
//!   -> PhysicalNode + partition info + fresh OperatorState
//!   -> StreamingExecutor (per-query, cheap)
//! ```
//!
//! Phase 2 pilot: Source / Filter / Project / Limit / Sort / HashJoin.
//! Phase 4: Exchange node for morsel-style partition execution.

use std::sync::Arc;

use super::super::executor::StreamingExecutor;
use super::super::operators::base::OperatorBase;
use super::super::operators::spec::{
    ApplySpec, DdlSpec, FulltextSpec, GraphSpec, RecursiveFragmentSpec, SetSpec, SinkSpec,
    TxnSpec, VectorSpec,
};
use super::super::operators::spec::{BlockingSpec, ExchangeSpec, JoinSpec, SourceSpec, UnarySpec};
use super::super::operators::apply_operator::ApplyOperator;
use super::super::operators::blocking_operator::BlockingOperator;
use super::super::operators::ddl_operator::DdlOperator;
use super::super::operators::exchange_operator::ExchangeOperator;
use super::super::operators::fulltext_operator::FulltextOperator;
use super::super::operators::graph_operator::GraphOperator;
use super::super::operators::join_operator::JoinOperator;
use super::super::operators::recursive_fragment_operator::RecursiveFragmentOperator;
use super::super::operators::set_operator::SetOperator;
use super::super::operators::sink_operator::SinkOperator;
use super::super::operators::source_operator::SourceOperator;
use super::super::operators::txn_operator::TxnOperator;
use super::super::operators::unary_operator::UnaryOperator;
use super::super::operators::vector_operator::VectorOperator;
use super::properties::PhysicalProperties;
use super::super::runtime::ExecutionRuntime;
use crate::query::executor::base::MemoryBudget;

/// Stable identifier for a physical plan node.
pub type PhysicalNodeId = i64;

/// Immutable physical plan node.
///
/// Every variant carries a stable [`PhysicalNodeId`] that matches the source
/// logical plan's node id, enabling PROFILE and EXPLAIN to report accurate
/// per-node metrics.
///
/// Contains only immutable configuration — expressions, column names, storage
/// handles — and never cursors, hash tables, lifecycle markers, or runtime
/// references.
#[derive(Debug, Clone)]
pub enum PhysicalNode {
    Source(PhysicalNodeId, SourceSpec, PhysicalProperties),
    Unary(
        PhysicalNodeId,
        Box<PhysicalNode>,
        UnarySpec,
        PhysicalProperties,
    ),
    Blocking(
        PhysicalNodeId,
        Box<PhysicalNode>,
        BlockingSpec,
        PhysicalProperties,
    ),
    Join(
        PhysicalNodeId,
        Box<PhysicalNode>,
        Box<PhysicalNode>,
        JoinSpec,
        PhysicalProperties,
    ),
    Graph(
        PhysicalNodeId,
        Box<PhysicalNode>,
        GraphSpec,
        PhysicalProperties,
    ),
    /// RecursiveFragment: variable-length path, BFS, shortest-path, all-paths.
    RecursiveFragment(
        PhysicalNodeId,
        Box<PhysicalNode>,
        RecursiveFragmentSpec,
        PhysicalProperties,
    ),
    Sink(
        PhysicalNodeId,
        Box<PhysicalNode>,
        SinkSpec,
        PhysicalProperties,
    ),
    Set(
        PhysicalNodeId,
        Box<PhysicalNode>,
        Box<PhysicalNode>,
        SetSpec,
        PhysicalProperties,
    ),
    Apply(
        PhysicalNodeId,
        Box<PhysicalNode>,
        Box<PhysicalNode>,
        ApplySpec,
        PhysicalProperties,
    ),
    /// Exchange node: gathers N child partition outputs via Concatenate or MergeSort.
    Exchange(
        PhysicalNodeId,
        Vec<PhysicalNode>,
        ExchangeSpec,
        PhysicalProperties,
    ),
    Ddl(
        PhysicalNodeId,
        Box<PhysicalNode>,
        DdlSpec,
        PhysicalProperties,
    ),
    Fulltext(
        PhysicalNodeId,
        Box<PhysicalNode>,
        FulltextSpec,
        PhysicalProperties,
    ),
    Vector(
        PhysicalNodeId,
        Box<PhysicalNode>,
        VectorSpec,
        PhysicalProperties,
    ),
    Txn(
        PhysicalNodeId,
        Box<PhysicalNode>,
        TxnSpec,
        PhysicalProperties,
    ),
}

impl PhysicalNode {
    /// Materialize this physical plan into a `StreamingExecutor` with fresh
    /// mutable state.  Each call creates independent operator state so the
    /// same `PhysicalNode` tree can be used for concurrent query executions.
    pub fn materialize(
        &self,
        runtime: Option<Arc<ExecutionRuntime>>,
        memory_budget: &MemoryBudget,
        chunk_size: usize,
    ) -> StreamingExecutor {
        let mut exec = match self {
            Self::Source(node_id, spec, _) => {
                let storage = runtime.as_ref().and_then(|rt| rt.storage.clone());
                let source = SourceOperator::from_spec(spec, storage);
                StreamingExecutor::Source(OperatorBase::new(*node_id), source)
            }
            Self::Unary(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let unary = UnaryOperator::from_spec(spec);
                StreamingExecutor::Unary(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    unary,
                )
            }
            Self::Blocking(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let blocking = BlockingOperator::from_spec(spec, memory_budget);
                StreamingExecutor::Blocking(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    blocking,
                )
            }
            Self::Join(node_id, left, right, spec, _) => {
                let left_exec = left.materialize(runtime.clone(), memory_budget, chunk_size);
                let right_exec = right.materialize(runtime.clone(), memory_budget, chunk_size);
                let join = JoinOperator::from_spec(spec, memory_budget);
                StreamingExecutor::Join(
                    OperatorBase::new(*node_id),
                    Box::new(left_exec),
                    Box::new(right_exec),
                    join,
                )
            }
            Self::Graph(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let storage = runtime.as_ref().and_then(|runtime| runtime.storage.clone());
                let space_name = runtime
                    .as_ref()
                    .and_then(|runtime| runtime.query_id().space_name)
                    .unwrap_or_default();
                StreamingExecutor::Graph(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    GraphOperator::from_spec(spec, storage, space_name),
                )
            }
            Self::RecursiveFragment(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let storage = runtime.as_ref().and_then(|rt| rt.storage.clone());
                let space_name = runtime
                    .as_ref()
                    .and_then(|rt| rt.query_id().space_name)
                    .unwrap_or_default();
                StreamingExecutor::RecursiveFragment(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    RecursiveFragmentOperator::from_spec(spec, storage, space_name),
                )
            }
            Self::Sink(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let storage = runtime.as_ref().and_then(|rt| rt.storage.clone());
                StreamingExecutor::Sink(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    SinkOperator::from_spec(spec, storage),
                )
            }
            Self::Set(node_id, left, right, spec, _) => {
                let left_exec = left.materialize(runtime.clone(), memory_budget, chunk_size);
                let right_exec = right.materialize(runtime.clone(), memory_budget, chunk_size);
                StreamingExecutor::Set(
                    OperatorBase::new(*node_id),
                    Box::new(left_exec),
                    Box::new(right_exec),
                    SetOperator::from_spec(spec, memory_budget),
                )
            }
            Self::Apply(node_id, left, right, spec, _) => {
                let left_exec = left.materialize(runtime.clone(), memory_budget, chunk_size);
                let right_exec = right.materialize(runtime.clone(), memory_budget, chunk_size);
                StreamingExecutor::Apply(
                    OperatorBase::new(*node_id),
                    Box::new(left_exec),
                    Box::new(right_exec),
                    ApplyOperator::from_spec(spec, memory_budget),
                )
            }
            Self::Exchange(node_id, children, spec, _) => {
                let child_execs: Vec<StreamingExecutor> = children
                    .iter()
                    .map(|c| c.materialize(runtime.clone(), memory_budget, chunk_size))
                    .collect();
                StreamingExecutor::Exchange(
                    OperatorBase::new(*node_id),
                    child_execs,
                    ExchangeOperator::from_spec(spec),
                )
            }
            Self::Ddl(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let storage = runtime.as_ref().and_then(|runtime| runtime.storage.clone());
                StreamingExecutor::Ddl(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    DdlOperator::from_spec(spec, storage),
                )
            }
            Self::Fulltext(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let storage = runtime.as_ref().and_then(|runtime| runtime.storage.clone());
                #[cfg(feature = "fulltext-search")]
                let fulltext_manager = runtime
                    .as_ref()
                    .and_then(|runtime| runtime.fulltext_manager.clone());
                StreamingExecutor::Fulltext(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    FulltextOperator::from_spec(
                        spec,
                        storage,
                        #[cfg(feature = "fulltext-search")]
                        fulltext_manager,
                    ),
                )
            }
            Self::Vector(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let storage = runtime.as_ref().and_then(|runtime| runtime.storage.clone());
                #[cfg(feature = "qdrant")]
                let vector_coordinator = runtime
                    .as_ref()
                    .and_then(|runtime| runtime.vector_coordinator.clone());
                StreamingExecutor::Vector(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    VectorOperator::from_spec(
                        spec,
                        storage,
                        #[cfg(feature = "qdrant")]
                        vector_coordinator,
                    ),
                )
            }
            Self::Txn(node_id, child, spec, _) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                StreamingExecutor::Txn(
                    OperatorBase::new(*node_id),
                    Box::new(child_exec),
                    TxnOperator::from_spec(spec),
                )
            }
        };
        exec.set_chunk_size(chunk_size);
        if let Some(rt) = runtime {
            exec.set_runtime(Some(rt));
        }
        exec
    }

    /// Return child nodes for tree traversal.
    pub fn children(&self) -> Vec<&PhysicalNode> {
        match self {
            Self::Source(..) => vec![],
            Self::Unary(_, child, _, _)
            | Self::Blocking(_, child, _, _)
            | Self::Graph(_, child, _, _)
            | Self::RecursiveFragment(_, child, _, _)
            | Self::Sink(_, child, _, _)
            | Self::Ddl(_, child, _, _)
            | Self::Fulltext(_, child, _, _)
            | Self::Vector(_, child, _, _)
            | Self::Txn(_, child, _, _) => {
                vec![child.as_ref()]
            }
            Self::Join(_, left, right, _, _)
            | Self::Set(_, left, right, _, _)
            | Self::Apply(_, left, right, _, _) => vec![left.as_ref(), right.as_ref()],
            Self::Exchange(_, children, _, _) => children.iter().collect(),
        }
    }
}
