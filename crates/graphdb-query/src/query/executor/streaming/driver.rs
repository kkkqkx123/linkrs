//! ExecutorDriver: wraps operator open/next/close with runtime context.
//!
//! In the enum-based executor the operator `open/next/close` signatures
//! do not (yet) carry an `ExecutionRuntime` parameter.  `ExecutorDriver`
//! bridges that gap by providing uniform cancel-checking, profiling, and
//! resource-tracking around each operator call.
//!
//! Future phase: once all operators accept runtime, this driver becomes
//! a thin delegation layer (or is inlined into the engine).
//!
//! Phase 1 contract:
//! - cancel check before each operator method call
//! - profile instrumentation (open/next/close time, input/output rows, peak memory)
//! - resource cleanup via runtime on close

use std::sync::Arc;
use std::time::Instant;

use super::chunk::DataChunk;
use super::executor::StreamingExecutor;
use super::runtime::{ExecutionRuntime, OperatorProfile};
use crate::core::error::QueryError;

/// Driver that wraps operator lifecycle with runtime context.
///
/// Every call to `open`/`next`/`close` on an operator is routed through
/// the driver, which adds cancel checking, profiling, and resource
/// management.
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
        let result = executor.next()?;
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
        let node_id = extract_plan_node_id(executor);
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
        let node_id = extract_plan_node_id(executor);
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
        let node_id = extract_plan_node_id(executor);
        let mut profile = self.runtime.profile().lock();
        if let Some(entry) = profile.operators.get_mut(&node_id) {
            entry.peak_memory = entry.peak_memory.max(peak_bytes);
        }
    }

    /// Convenience: check cancel inside a long-running operator loop.
    /// Returns an error if the query has been cancelled.
    pub fn check_cancel(&self) -> Result<(), QueryError> {
        self.runtime.ensure_not_cancelled()
    }
}

fn extract_plan_node_id(executor: &StreamingExecutor) -> i64 {
    use StreamingExecutor::*;
    match executor {
        ScanVertices { plan_node_id, .. } => *plan_node_id,
        StorageScanVertices { plan_node_id, .. } => *plan_node_id,
        ScanEdges { plan_node_id, .. } => *plan_node_id,
        StorageScanEdges { plan_node_id, .. } => *plan_node_id,
        Filter { plan_node_id, .. } => *plan_node_id,
        Project { plan_node_id, .. } => *plan_node_id,
        Limit { plan_node_id, .. } => *plan_node_id,
        Sort { plan_node_id, .. } => *plan_node_id,
        Aggregate { plan_node_id, .. } => *plan_node_id,
        HashJoin { plan_node_id, .. } => *plan_node_id,
        InnerJoin { plan_node_id, .. } => *plan_node_id,
        LeftJoin { plan_node_id, .. } => *plan_node_id,
        RightJoin { plan_node_id, .. } => *plan_node_id,
        FullOuterJoin { plan_node_id, .. } => *plan_node_id,
        CrossJoin { plan_node_id, .. } => *plan_node_id,
        SemiJoin { plan_node_id, .. } => *plan_node_id,
        Union { plan_node_id, .. } => *plan_node_id,
        Intersect { plan_node_id, .. } => *plan_node_id,
        Except { plan_node_id, .. } => *plan_node_id,
        Minus { plan_node_id, .. } => *plan_node_id,
        // Feature-gated search variants (return 0 as fallback)
        #[cfg(feature = "fulltext-search")]
        FulltextSearch { .. }
        | FulltextLookup { .. }
        | MatchFulltext { .. } => 0,
        #[cfg(feature = "qdrant")]
        VectorSearch { .. }
        | VectorLookup { .. }
        | VectorMatch { .. } => 0,
        _ => 0,
    }
}

fn extract_operator_name(executor: &StreamingExecutor) -> String {
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
        _ => "Unknown".to_string(),
    }
}
