//! Single-threaded pipeline runner
//!
//! Executes a PipelineGraph in a single thread by processing pipelines
//! in topological order and materializing results at breaker boundaries.

use std::sync::Arc;

use super::graph::PipelineGraph;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::builder::StreamingExecutorBuilder;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::engine::StreamingExecutionEngine;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

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
    /// Replaces upstream plan nodes with in-memory scans using the
    /// materialized output from previously-executed pipelines.  This
    /// enables correct per-pipeline execution at breaker boundaries.
    fn build_executor_with_materialized_inputs(
        node: &PlanNodeEnum,
        upstream_ids: &[usize],
        outputs: &[Option<Vec<DataChunk>>],
        context: &ExecutionContext,
    ) -> Result<StreamingExecutor, QueryError> {
        // Collect materialized rows from upstream outputs
        let mut upstream_rows: Vec<Vec<Value>> = Vec::new();
        let mut upstream_col_names: Vec<String> = Vec::new();
        for &up_id in upstream_ids {
            if let Some(Some(chunks)) = outputs.get(up_id) {
                for chunk in chunks {
                    if upstream_col_names.is_empty() {
                        upstream_col_names = chunk.col_names();
                    }
                    upstream_rows.extend(chunk.rows.clone());
                }
            }
        }

        if upstream_rows.is_empty() {
            return Self::build_executor(node, context);
        }

        // If the node has a single input, replace that input with a scan
        // of the materialized data.  Otherwise fall back to just the scan.
        if Self::try_get_single_input(node).is_some() {
            let input_executor = StreamingExecutorBuilder::build_simple_scan_with_col_names(
                upstream_rows,
                upstream_col_names,
            )?;
            // Build the full executor, then replace leaf with materialized scan
            let mut executor = StreamingExecutorBuilder::from_plan_node(node, context)?;
            Self::replace_leaf_executor(&mut executor, input_executor);
            Ok(executor)
        } else {
            // Fallback: just return the materialized data as a scan
            StreamingExecutorBuilder::build_simple_scan_with_col_names(
                upstream_rows,
                upstream_col_names,
            )
        }
    }

    /// Try to get the single input of a plan node, if it has exactly one.
    fn try_get_single_input(node: &PlanNodeEnum) -> Option<&PlanNodeEnum> {
        match node {
            PlanNodeEnum::Filter(n) => Some(n.input()),
            PlanNodeEnum::Project(n) => Some(n.input()),
            PlanNodeEnum::Limit(n) => Some(n.input()),
            PlanNodeEnum::Sort(n) => Some(n.input()),
            PlanNodeEnum::TopN(n) => Some(n.input()),
            PlanNodeEnum::Sample(n) => Some(n.input()),
            PlanNodeEnum::Aggregate(n) => Some(n.input()),
            PlanNodeEnum::Dedup(n) => Some(n.input()),
            PlanNodeEnum::Window(n) => Some(n.input()),
            PlanNodeEnum::Traverse(n) => Some(n.input()),
            PlanNodeEnum::Materialize(n) => n.dependencies().first(),
            PlanNodeEnum::DataCollect(n) => n.dependencies().first(),
            PlanNodeEnum::Unwind(n) => n.dependencies().first(),
            PlanNodeEnum::Assign(n) => n.dependencies().first(),
            _ => None,
        }
    }

    /// Walk the executor tree and replace the first leaf executor with the given one.
    fn replace_leaf_executor(executor: &mut StreamingExecutor, replacement: StreamingExecutor) {
        match executor {
            // Operators with a single child: recurse
            StreamingExecutor::Unary(_, input, _)
            | StreamingExecutor::Blocking(_, input, _)
            | StreamingExecutor::Graph(_, input, _)
            | StreamingExecutor::Sink(_, input, _)
            | StreamingExecutor::Ddl(_, input, _)
            | StreamingExecutor::Fulltext(_, input, _)
            | StreamingExecutor::Vector(_, input, _)
            | StreamingExecutor::Txn(_, input, _) => {
                Self::replace_leaf_executor(input, replacement);
            }
            // Join/Set/Apply: replace leaves in left child only (first leaf found)
            StreamingExecutor::Join(_, left, _, _)
            | StreamingExecutor::Set(_, left, _, _)
            | StreamingExecutor::Apply(_, left, _, _) => {
                Self::replace_leaf_executor(left, replacement);
            }
            // Leaf executors (Source has no children): replace
            StreamingExecutor::Source(..) => {
                *executor = replacement;
            }
        }
    }

    /// Generate explain output for this pipeline graph
    pub fn explain(&self) -> String {
        self.graph.explain()
    }
}
