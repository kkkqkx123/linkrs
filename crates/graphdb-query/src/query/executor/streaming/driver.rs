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
        ScanVertices { .. } | StorageScanVertices { .. } => "ScanVertices".to_string(),
        ScanEdges { .. } | StorageScanEdges { .. } => "ScanEdges".to_string(),
        Filter { .. } => "Filter".to_string(),
        Project { .. } => "Project".to_string(),
        Limit { .. } => "Limit".to_string(),
        Sort { .. } => "Sort".to_string(),
        Aggregate { .. } => "Aggregate".to_string(),
        HashJoin { .. } => "HashJoin".to_string(),
        HashLeftJoin { .. } => "HashLeftJoin".to_string(),
        InnerJoin { .. } => "InnerJoin".to_string(),
        LeftJoin { .. } => "LeftJoin".to_string(),
        RightJoin { .. } => "RightJoin".to_string(),
        FullOuterJoin { .. } => "FullOuterJoin".to_string(),
        CrossJoin { .. } => "CrossJoin".to_string(),
        SemiJoin { .. } => "SemiJoin".to_string(),
        NestedLoopJoin { .. } => "NestedLoopJoin".to_string(),
        Union { .. } => "Union".to_string(),
        UnionAll { .. } => "UnionAll".to_string(),
        Intersect { .. } => "Intersect".to_string(),
        Except { .. } => "Except".to_string(),
        Minus { .. } => "Minus".to_string(),
        Expand { .. } => "Expand".to_string(),
        ExpandAll { .. } => "ExpandAll".to_string(),
        Traverse { .. } => "Traverse".to_string(),
        TraverseAll { .. } => "TraverseAll".to_string(),
        ShortestPath { .. } => "ShortestPath".to_string(),
        BFSShortest { .. } => "BFSShortest".to_string(),
        AllPaths { .. } => "AllPaths".to_string(),
        MultiShortestPath { .. } => "MultiShortestPath".to_string(),
        Subgraph { .. } => "Subgraph".to_string(),
        Start { .. } => "Start".to_string(),
        GetVertices { .. } => "GetVertices".to_string(),
        GetEdges { .. } => "GetEdges".to_string(),
        GetNeighbors { .. } => "GetNeighbors".to_string(),
        EdgeIndexScan { .. } => "EdgeIndexScan".to_string(),
        IndexScan { .. } => "IndexScan".to_string(),
        Argument { .. } => "Argument".to_string(),
        Sample { .. } => "Sample".to_string(),
        GetProp { .. } => "GetProp".to_string(),
        LookupIndex { .. } => "LookupIndex".to_string(),
        Dedup { .. } => "Dedup".to_string(),
        TopN { .. } => "TopN".to_string(),
        Assign { .. } => "Assign".to_string(),
        Materialize { .. } => "Materialize".to_string(),
        Remove { .. } => "Remove".to_string(),
        DataCollect { .. } => "DataCollect".to_string(),
        Unwind { .. } => "Unwind".to_string(),
        Apply { .. } => "Apply".to_string(),
        PatternApply { .. } => "PatternApply".to_string(),
        RollUpApply { .. } => "RollUpApply".to_string(),
        Window { .. } => "Window".to_string(),
        GroupBy { .. } => "GroupBy".to_string(),
        Distinct { .. } => "Distinct".to_string(),
        WindowFunction { .. } => "WindowFunction".to_string(),
        AppendVertices { .. } => "AppendVertices".to_string(),
        BiExpand { .. } => "BiExpand".to_string(),
        BiTraverse { .. } => "BiTraverse".to_string(),
        InsertVertices { .. } => "InsertVertices".to_string(),
        InsertEdges { .. } => "InsertEdges".to_string(),
        UpdateVertices { .. } => "UpdateVertices".to_string(),
        UpdateEdges { .. } => "UpdateEdges".to_string(),
        DeleteVertices { .. } => "DeleteVertices".to_string(),
        DeleteEdges { .. } => "DeleteEdges".to_string(),
        PipeDeleteVertices { .. } => "PipeDeleteVertices".to_string(),
        PipeDeleteEdges { .. } => "PipeDeleteEdges".to_string(),
        DeleteTags { .. } => "DeleteTags".to_string(),
        FulltextSearch { .. } => "FulltextSearch".to_string(),
        FulltextLookup { .. } => "FulltextLookup".to_string(),
        MatchFulltext { .. } => "MatchFulltext".to_string(),
        VectorSearch { .. } => "VectorSearch".to_string(),
        VectorLookup { .. } => "VectorLookup".to_string(),
        VectorMatch { .. } => "VectorMatch".to_string(),
        SpaceManage { .. } => "SpaceManage".to_string(),
        TagManage { .. } => "TagManage".to_string(),
        EdgeManage { .. } => "EdgeManage".to_string(),
        IndexManage { .. } => "IndexManage".to_string(),
        UserManage { .. } => "UserManage".to_string(),
        FulltextManage { .. } => "FulltextManage".to_string(),
        VectorManage { .. } => "VectorManage".to_string(),
        Loop { .. } => "Loop".to_string(),
        Select { .. } => "Select".to_string(),
        PassThrough { .. } => "PassThrough".to_string(),
        BeginTransaction { .. } => "BeginTransaction".to_string(),
        Commit { .. } => "Commit".to_string(),
        Rollback { .. } => "Rollback".to_string(),
        ShowStats { .. } => "ShowStats".to_string(),
        Analyze { .. } => "Analyze".to_string(),
        Migrate { .. } => "Migrate".to_string(),
    }
}
