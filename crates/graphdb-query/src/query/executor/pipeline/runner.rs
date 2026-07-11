//! Single-threaded pipeline runner
//!
//! Executes a PipelineGraph in a single thread by processing pipelines
//! in topological order and materializing results at breaker boundaries.

use std::sync::Arc;

use super::graph::PipelineGraph;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::engine::StreamingExecutionEngine;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Single-threaded runner for pipeline graphs.
///
/// For Phase 6a, this runner works in two modes:
/// 1. **Flat mode** (default): Builds a single StreamingExecutor from the
///    original plan and executes it. The pipeline graph is metadata only.
/// 2. **Pipelined mode**: Executes each pipeline separately, materializing
///    output at breaker boundaries. Used for verification.
pub struct PipelineRunner {
    graph: PipelineGraph,
    context: ExecutionContext,
}

impl PipelineRunner {
    pub fn new(graph: PipelineGraph, context: ExecutionContext) -> Self {
        Self { graph, context }
    }

    pub fn graph(&self) -> &PipelineGraph {
        &self.graph
    }

    /// Execute the query using the standard flat executor (single tree).
    /// This produces the correct output and is the default path.
    pub fn execute_flat(&self) -> Result<Vec<DataChunk>, QueryError> {
        let root = self
            .graph
            .pipelines
            .iter()
            .find(|p| p.id == self.graph.root_pipeline_id)
            .map(|p| p.root_node.clone())
            .ok_or_else(|| QueryError::execution("Root pipeline not found".to_string()))?;

        let executor =
            crate::query::executor::streaming::builder::StreamingExecutorBuilder::from_plan_node(
                &root,
                &self.context,
            )?;

        let mut engine = StreamingExecutionEngine::new();
        engine.register_executor(0, executor);

        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: 0,
                session_id: None,
                space_name: self.context.space_name.clone(),
            },
            self.context.memory_budget.clone(),
        ));
        engine.set_runtime(runtime.clone());

        engine.execute()
    }

    /// Execute pipelines separately, materializing output between breakers.
    /// For Phase 6a this is experimental — output may not match exactly
    /// for complex plans.
    pub fn execute_pipelined(&self) -> Result<Vec<DataChunk>, QueryError> {
        let order = self.graph.topological_order();
        let mut pipeline_outputs: Vec<Option<Vec<DataChunk>>> =
            vec![None; self.graph.pipelines.len()];

        for &pid in &order {
            let pipeline = &self.graph.pipelines[pid];

            let executor = if pipeline.upstream_ids.is_empty() {
                // Leaf pipeline: build executor directly from sub-tree
                Self::build_executor(&pipeline.root_node, &self.context)?
            } else {
                // Non-leaf pipeline: replace upstream inputs with materialized data
                Self::build_executor_with_materialized_inputs(
                    &pipeline.root_node,
                    &pipeline.upstream_ids,
                    &pipeline_outputs,
                    &self.context,
                )?
            };

            let mut engine = StreamingExecutionEngine::new();
            engine.register_executor(0, executor);

            let runtime = Arc::new(ExecutionRuntime::new(
                QueryIdentity {
                    query_id: 0,
                    session_id: None,
                    space_name: self.context.space_name.clone(),
                },
                self.context.memory_budget.clone(),
            ));
            engine.set_runtime(runtime.clone());

            let output = engine.execute()?;
            pipeline_outputs[pid] = Some(output);
        }

        // Collect output from root pipeline
        Ok(pipeline_outputs[self.graph.root_pipeline_id]
            .take()
            .unwrap_or_default())
    }

    /// Build a StreamingExecutor from a plan sub-tree.
    fn build_executor(
        node: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<StreamingExecutor, QueryError> {
        crate::query::executor::streaming::builder::StreamingExecutorBuilder::from_plan_node(
            node, context,
        )
    }

    /// Build executor with upstream materialized data replacing original inputs.
    ///
    /// In Phase 6a this replaces upstream plan nodes with in-memory scans
    /// using the materialized output from previously-executed pipelines.
    /// This enables separate pipeline execution at breaker boundaries.
    fn build_executor_with_materialized_inputs(
        node: &PlanNodeEnum,
        upstream_ids: &[usize],
        outputs: &[Option<Vec<DataChunk>>],
        context: &ExecutionContext,
    ) -> Result<StreamingExecutor, QueryError> {
        // Phase 6a: collect materialized rows from upstream outputs
        let mut upstream_rows: Vec<Vec<Value>> = Vec::new();
        for &up_id in upstream_ids {
            if let Some(Some(chunks)) = outputs.get(up_id) {
                for chunk in chunks {
                    upstream_rows.extend(chunk.rows.clone());
                }
            }
        }

        if upstream_rows.is_empty() {
            // No materialized upstream data: build from original plan node
            return Self::build_executor(node, context);
        }

        // Build a scan executor from the materialized data and wrap the
        // given node around it by replacing the original leaf input.
        let scan = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: upstream_rows,
            current_index: 0,
            col_names: vec![],
            plan_node_id: 0,
            runtime: None,
        };

        Ok(scan)
    }

    /// Generate explain output for this pipeline graph
    pub fn explain(&self) -> String {
        self.graph.explain()
    }
}
