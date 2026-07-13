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
//! All other node types still use the legacy `StreamingExecutorBuilder` path
//! and bypass `PhysicalNode` entirely.

use std::sync::Arc;

use super::executor::StreamingExecutor;
use super::operator_base::OperatorBase;
use super::operator_spec::{BlockingSpec, JoinSpec, SourceSpec, UnarySpec};
use super::operator_spec::{ApplySpec, GraphSpec, SetSpec, SinkSpec};
use super::operators::apply_operator::ApplyOperator;
use super::operators::blocking_operator::BlockingOperator;
use super::operators::graph_operator::GraphOperator;
use super::operators::join_operator::JoinOperator;
use super::operators::set_operator::SetOperator;
use super::operators::sink_operator::SinkOperator;
use super::operators::source_operator::SourceOperator;
use super::operators::unary_operator::UnaryOperator;
use super::runtime::ExecutionRuntime;
use crate::query::executor::base::MemoryBudget;

/// Immutable physical plan node for Phase 2 pilot operators.
///
/// Contains only immutable configuration — expressions, column names, storage
/// handles — and never cursors, hash tables, lifecycle markers, or runtime
/// references.
#[derive(Debug, Clone)]
pub enum PhysicalNode {
    Source(SourceSpec),
    Unary(Box<PhysicalNode>, UnarySpec),
    Blocking(Box<PhysicalNode>, BlockingSpec),
    Join(Box<PhysicalNode>, Box<PhysicalNode>, JoinSpec),
    Graph(Box<PhysicalNode>, GraphSpec),
    Sink(Box<PhysicalNode>, SinkSpec),
    Set(Box<PhysicalNode>, Box<PhysicalNode>, SetSpec),
    Apply(Box<PhysicalNode>, Box<PhysicalNode>, ApplySpec),
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
        match self {
            Self::Source(spec) => {
                let source = SourceOperator::from_spec(spec);
                let mut exec = StreamingExecutor::Source(OperatorBase::new(0), source);
                exec.set_chunk_size(chunk_size);
                if let Some(rt) = runtime {
                    exec.set_runtime(Some(rt));
                }
                exec
            }
            Self::Unary(child, spec) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let unary = UnaryOperator::from_spec(spec);
                let mut exec =
                    StreamingExecutor::Unary(OperatorBase::new(0), Box::new(child_exec), unary);
                exec.set_chunk_size(chunk_size);
                if let Some(rt) = runtime {
                    exec.set_runtime(Some(rt));
                }
                exec
            }
            Self::Blocking(child, spec) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let blocking = BlockingOperator::from_spec(spec, memory_budget);
                let mut exec = StreamingExecutor::Blocking(
                    OperatorBase::new(0),
                    Box::new(child_exec),
                    blocking,
                );
                exec.set_chunk_size(chunk_size);
                if let Some(rt) = runtime {
                    exec.set_runtime(Some(rt));
                }
                exec
            }
            Self::Join(left, right, spec) => {
                let left_exec = left.materialize(runtime.clone(), memory_budget, chunk_size);
                let right_exec = right.materialize(runtime.clone(), memory_budget, chunk_size);
                let join = JoinOperator::from_spec(spec, memory_budget);
                let mut exec = StreamingExecutor::Join(
                    OperatorBase::new(0),
                    Box::new(left_exec),
                    Box::new(right_exec),
                    join,
                );
                exec.set_chunk_size(chunk_size);
                if let Some(rt) = runtime {
                    exec.set_runtime(Some(rt));
                }
                exec
            }
            Self::Graph(child, spec) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let mut exec = StreamingExecutor::Graph(
                    OperatorBase::new(0),
                    Box::new(child_exec),
                    GraphOperator::from_spec(spec),
                );
                exec.set_chunk_size(chunk_size);
                if let Some(rt) = runtime {
                    exec.set_runtime(Some(rt));
                }
                exec
            }
            Self::Sink(child, spec) => {
                let child_exec = child.materialize(runtime.clone(), memory_budget, chunk_size);
                let mut exec = StreamingExecutor::Sink(
                    OperatorBase::new(0),
                    Box::new(child_exec),
                    SinkOperator::from_spec(spec),
                );
                exec.set_chunk_size(chunk_size);
                if let Some(rt) = runtime {
                    exec.set_runtime(Some(rt));
                }
                exec
            }
            Self::Set(left, right, spec) => {
                let left_exec = left.materialize(runtime.clone(), memory_budget, chunk_size);
                let right_exec = right.materialize(runtime.clone(), memory_budget, chunk_size);
                let mut exec = StreamingExecutor::Set(
                    OperatorBase::new(0),
                    Box::new(left_exec),
                    Box::new(right_exec),
                    SetOperator::from_spec(spec, memory_budget),
                );
                exec.set_chunk_size(chunk_size);
                if let Some(rt) = runtime {
                    exec.set_runtime(Some(rt));
                }
                exec
            }
            Self::Apply(left, right, spec) => {
                let left_exec = left.materialize(runtime.clone(), memory_budget, chunk_size);
                let right_exec = right.materialize(runtime.clone(), memory_budget, chunk_size);
                let mut exec = StreamingExecutor::Apply(
                    OperatorBase::new(0),
                    Box::new(left_exec),
                    Box::new(right_exec),
                    ApplyOperator::from_spec(spec, memory_budget),
                );
                exec.set_chunk_size(chunk_size);
                if let Some(rt) = runtime {
                    exec.set_runtime(Some(rt));
                }
                exec
            }
        }
    }

    /// Return child nodes for tree traversal.
    pub fn children(&self) -> Vec<&PhysicalNode> {
        match self {
            Self::Source(_) => vec![],
            Self::Unary(child, _) | Self::Blocking(child, _) | Self::Graph(child, _) | Self::Sink(child, _) => {
                vec![child.as_ref()]
            }
            Self::Join(left, right, _)
            | Self::Set(left, right, _)
            | Self::Apply(left, right, _) => vec![left.as_ref(), right.as_ref()],
        }
    }
}
