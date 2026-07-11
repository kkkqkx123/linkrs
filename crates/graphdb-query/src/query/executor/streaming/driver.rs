//! ExecutorDriver: wraps operator open/next/close with runtime context.
//!
//! Phase 3: operator dispatch in executor.rs now handles cancel-checking,
//! profiling, and resource tracking directly via the injected runtime.
//! ExecutorDriver is retained as a thin compatibility layer for engine.rs
//! but its per-call wrapping is redundant when the runtime is attached.
//!
//! Future: remove ExecutorDriver entirely once all paths go through
//! the operator dispatch in executor.rs.

use std::sync::Arc;
use std::time::Instant;

use super::chunk::DataChunk;
use super::executor::StreamingExecutor;

use super::operators::apply_operator::ApplyOperator;
use super::operators::blocking_operator::BlockingOperator;
use super::operators::ddl_operator::DdlOperator;
use super::operators::fulltext_operator::FulltextOperator;
use super::operators::graph_operator::GraphOperator;
use super::operators::join_operator::JoinOperator;
use super::operators::set_operator::SetOperator;
use super::operators::sink_operator::SinkOperator;
use super::operators::source_operator::SourceOperator;
use super::operators::txn_operator::TxnOperator;
use super::operators::unary_operator::UnaryOperator;
use super::operators::vector_operator::VectorOperator;
use super::runtime::{ExecutionRuntime, OperatorProfile};
use crate::core::error::QueryError;

/// Driver that wraps operator lifecycle with runtime context.
///
/// Phase 3: operator dispatch in executor.rs carries its own cancel
/// checking + profiling.  The driver is retained for engine.rs paths
/// that have not yet migrated.
#[derive(Debug)]
pub struct ExecutorDriver {
    runtime: Arc<ExecutionRuntime>,
}

impl ExecutorDriver {
    pub fn new(runtime: Arc<ExecutionRuntime>) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> &Arc<ExecutionRuntime> {
        &self.runtime
    }

    /// Open an operator, wrapping with runtime checks and profiling.
    pub fn open(&self, executor: &mut StreamingExecutor) -> Result<(), QueryError> {
        self.runtime.ensure_not_cancelled()?;
        let start = Instant::now();
        executor.open()?;
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_timing(executor, "open", elapsed);
        Ok(())
    }

    /// Pull the next chunk, wrapping with runtime checks and profiling.
    pub fn next(
        &self,
        executor: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        self.runtime.ensure_not_cancelled()?;
        let start = Instant::now();
        let result = executor.advance()?;
        let elapsed = start.elapsed().as_micros() as u64;
        if let Some(ref chunk) = result {
            let count = chunk.len() as u64;
            self.runtime.profile_add_rows(count);
            self.record_row_count(executor, count, true);
        }
        self.record_timing(executor, "next", elapsed);
        Ok(result)
    }

    /// Close an operator, wrapping with runtime checks and profiling.
    ///
    /// Ensures resources are released even on error.
    pub fn close(&self, executor: &mut StreamingExecutor) -> Result<(), QueryError> {
        let start = Instant::now();
        let result = executor.close();
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_timing(executor, "close", elapsed);
        self.runtime.release_resources();
        result
    }

    // ── Profile helpers ──

    fn record_timing(&self, executor: &StreamingExecutor, phase: &str, elapsed_us: u64) {
        let node_id = executor.plan_node_id();
        let name = extract_operator_name(executor);
        let mut profile = self.runtime.profile().lock();
        let entry = profile.operators.entry(node_id).or_insert_with(|| OperatorProfile {
            node_id,
            name,
            ..OperatorProfile::default()
        });
        match phase {
            "open" => entry.open_time_us += elapsed_us,
            "next" => entry.next_time_us += elapsed_us,
            "close" => entry.close_time_us += elapsed_us,
            _ => {}
        }
    }

    fn record_row_count(&self, executor: &StreamingExecutor, count: u64, is_output: bool) {
        let node_id = executor.plan_node_id();
        let mut profile = self.runtime.profile().lock();
        if let Some(entry) = profile.operators.get_mut(&node_id) {
            if is_output {
                entry.output_rows += count;
            } else {
                entry.input_rows += count;
            }
        }
    }

    /// Record peak memory for an operator from a MemoryTracker.
    pub fn record_peak_memory(&self, executor: &StreamingExecutor, peak_bytes: u64) {
        let node_id = executor.plan_node_id();
        let mut profile = self.runtime.profile().lock();
        if let Some(entry) = profile.operators.get_mut(&node_id) {
            entry.peak_memory = entry.peak_memory.max(peak_bytes);
        }
    }

    /// Convenience: check cancel inside a long-running operator loop.
    pub fn check_cancel(&self) -> Result<(), QueryError> {
        self.runtime.ensure_not_cancelled()
    }
}



/// Extract operator name for all variants.
pub fn extract_operator_name(executor: &StreamingExecutor) -> String {
    use StreamingExecutor::*;
    match executor {
        Source(_, op) => match op {
            SourceOperator::ScanVertices { .. } | SourceOperator::StorageScanVertices { .. } => "ScanVertices",
            SourceOperator::ScanEdges { .. } | SourceOperator::StorageScanEdges { .. } => "ScanEdges",
            SourceOperator::GetVertices { .. } => "GetVertices",
            SourceOperator::GetEdges { .. } => "GetEdges",
            SourceOperator::GetNeighbors { .. } => "GetNeighbors",
            SourceOperator::EdgeIndexScan { .. } => "EdgeIndexScan",
            SourceOperator::IndexScan { .. } => "IndexScan",
            SourceOperator::Argument => "Argument",
            SourceOperator::GetProp { .. } => "GetProp",
            SourceOperator::LookupIndex { .. } => "LookupIndex",
            SourceOperator::Start => "Start",
        }.to_string(),
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
            UnaryOperator::Loop { .. } => "Loop",
            UnaryOperator::Select { .. } => "Select",
            UnaryOperator::PassThrough => "PassThrough",
        }.to_string(),
        Txn(_, _, op) => match op {
            TxnOperator::BeginTransaction { .. } => "BeginTransaction",
            TxnOperator::Commit { .. } => "Commit",
            TxnOperator::Rollback { .. } => "Rollback",
        }.to_string(),
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
        }.to_string(),
        Set(_, _, _, op) => match op {
            SetOperator::Union { .. } => "Union",
            SetOperator::UnionAll { .. } => "UnionAll",
            SetOperator::Intersect { .. } => "Intersect",
            SetOperator::Except { .. } => "Except",
            SetOperator::Minus { .. } => "Minus",
        }.to_string(),
        Apply(_, _, _, op) => match op {
            ApplyOperator::Apply { .. } => "Apply",
            ApplyOperator::PatternApply { .. } => "PatternApply",
        }.to_string(),
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
        }.to_string(),
        Graph(_, _, op) => match op {
            GraphOperator::Expand { .. } => "Expand",
            GraphOperator::ExpandAll { .. } => "ExpandAll",
            GraphOperator::Traverse { .. } => "Traverse",
            GraphOperator::TraverseAll { .. } => "TraverseAll",
            GraphOperator::BiExpand { .. } => "BiExpand",
            GraphOperator::BiTraverse { .. } => "BiTraverse",
            GraphOperator::ShortestPath { .. } => "ShortestPath",
            GraphOperator::BFSShortest { .. } => "BFSShortest",
            GraphOperator::AllPaths { .. } => "AllPaths",
            GraphOperator::MultiShortestPath { .. } => "MultiShortestPath",
            GraphOperator::Subgraph { .. } => "Subgraph",
        }.to_string(),
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
        }.to_string(),
        Ddl(_, _, op) => match op {
            DdlOperator::SpaceManage { .. } => "SpaceManage",
            DdlOperator::TagManage { .. } => "TagManage",
            DdlOperator::EdgeManage { .. } => "EdgeManage",
            DdlOperator::IndexManage { .. } => "IndexManage",
            DdlOperator::UserManage { .. } => "UserManage",
            DdlOperator::ShowStats { .. } => "ShowStats",
            DdlOperator::Analyze { .. } => "Analyze",
            DdlOperator::Migrate { .. } => "Migrate",
        }.to_string(),
        Fulltext(_, _, op) => match op {
            FulltextOperator::FulltextManage { .. } => "FulltextManage",
            FulltextOperator::FulltextSearch { .. } => "FulltextSearch",
            FulltextOperator::FulltextLookup { .. } => "FulltextLookup",
            FulltextOperator::MatchFulltext { .. } => "MatchFulltext",
        }.to_string(),
        Vector(_, _, op) => match op {
            VectorOperator::VectorManage { .. } => "VectorManage",
            VectorOperator::VectorSearch { .. } => "VectorSearch",
            VectorOperator::VectorLookup { .. } => "VectorLookup",
            VectorOperator::VectorMatch { .. } => "VectorMatch",
        }.to_string(),
    }
}
