//! High-level streaming execution interface.
//!
//! Provides [`StreamingQueryExecutor`] for creating and executing
//! streaming queries from execution plans.

use std::sync::Arc;

use super::builder::StreamingExecutorBuilder;
use super::chunk::DataChunk;
use super::engine::StreamingExecutionEngine;
use super::physical_builder::{self, BuildOutput};
use super::runtime::{ExecutionRuntime, QueryIdentity};
use super::stream::ResultStream;
use super::stream_result::StreamingQueryResult;
use crate::core::error::QueryError;
use crate::query::executor::base::{ExecutionContext, ExecutionResult};
use crate::query::planning::plan::{PartitionedPhysicalPlan, PlanNodeEnum};

use super::partition::PartitionView;

/// High-level streaming execution interface
///
/// Encapsulates the creation and execution of streaming queries
/// from a plan node without exposing executor complexity.
pub struct StreamingQueryExecutor {
    engine: Option<StreamingExecutionEngine>,
    col_names: Option<Vec<String>>,
    runtime: Option<Arc<ExecutionRuntime>>,
}

impl StreamingQueryExecutor {
    /// Create a new streaming executor
    pub fn new() -> Self {
        Self {
            engine: None,
            col_names: None,
            runtime: None,
        }
    }

    /// Build executor from a plan node
    pub fn from_plan_node(
        &mut self,
        plan_node: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<(), QueryError> {
        let executor = StreamingExecutorBuilder::from_plan_node(plan_node, context)?;

        let mut engine = StreamingExecutionEngine::new();
        engine.set_max_workers(context.max_workers);
        engine.set_max_buffered_chunks(context.max_buffered_chunks);
        engine.register_executor(0, executor);

        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: context.query_id,
                session_id: None,
                space_name: context.space_name.clone(),
            },
            context.memory_budget.clone(),
        ));
        engine.set_runtime(runtime.clone());

        self.engine = Some(engine);
        self.runtime = Some(runtime);
        Ok(())
    }

    /// Build a composable partitioned physical plan.
    pub fn from_partitioned_physical_plan(
        &mut self,
        physical_plan: &PartitionedPhysicalPlan,
        context: &ExecutionContext,
    ) -> Result<(), QueryError> {
        let partition_view = PartitionView::from(physical_plan.partition_spec());
        let mut next_gather_node_id = physical_builder::PHYSICAL_GATHER_NODE_ID_START;
        let root = physical_builder::build_partitioned_physical_node(
            physical_plan.root(),
            context,
            &partition_view,
            &mut next_gather_node_id,
        )?;

        let root = match root {
            BuildOutput::Global(executor) => executor,
            BuildOutput::Local(trees) => physical_builder::local_to_global(
                BuildOutput::Local(trees),
                &mut next_gather_node_id,
            )?,
        };

        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: context.query_id,
                session_id: None,
                space_name: context.space_name.clone(),
            },
            context.memory_budget.clone(),
        ));
        let mut engine = StreamingExecutionEngine::new();
        engine.set_max_workers(context.max_workers);
        engine.set_max_buffered_chunks(context.max_buffered_chunks);
        engine.set_runtime(runtime.clone());
        engine.register_partitioned_root(partition_view.partition_count, root)?;

        self.engine = Some(engine);
        self.runtime = Some(runtime);
        Ok(())
    }

    /// Execute the query and materialize all results into a DataSet.
    pub fn execute_materialized(&mut self) -> Result<ExecutionResult, QueryError> {
        let engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Streaming engine not initialized".to_string()))?;

        let stream = engine.into_stream()?;
        let dataset = stream.collect()?;
        Ok(ExecutionResult::DataSet {
            data: dataset,
            execution_mode_reason: None,
        })
    }

    /// Alias for `execute_materialized` — default path materializes the result.
    /// Use `execute_to_stream` for chunk-at-a-time streaming.
    pub fn execute(&mut self) -> Result<ExecutionResult, QueryError> {
        self.execute_materialized()
    }

    /// Execute the query and return a [`StreamingQueryResult`] for
    /// chunk-at-a-time consumption via SSE, gRPC, or other streaming protocols.
    pub fn execute_to_stream(&mut self) -> Result<StreamingQueryResult, QueryError> {
        let engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Streaming engine not initialized".to_string()))?;
        let runtime = self.runtime.take().ok_or_else(|| {
            QueryError::execution("Streaming runtime not initialized".to_string())
        })?;
        let stream = engine.into_stream()?;
        Ok(StreamingQueryResult::new(stream, runtime))
    }

    /// Run execution through the existing materialised path (collect all chunks).
    pub fn execute_collect(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let engine = self
            .engine
            .as_mut()
            .ok_or_else(|| QueryError::execution("Streaming engine not initialized".to_string()))?;
        engine.execute()
    }

    /// Return a [`ResultStream`] for chunk-at-a-time consumption.
    pub fn into_stream(&mut self) -> Result<ResultStream, QueryError> {
        let engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Streaming engine not initialized".to_string()))?;
        engine.into_stream()
    }

    /// Return a reference to the execution runtime, if set.
    pub fn runtime(&self) -> Option<&Arc<ExecutionRuntime>> {
        self.runtime.as_ref()
    }

    /// Set optional column names for result formatting
    pub fn set_col_names(&mut self, col_names: Vec<String>) {
        self.col_names = Some(col_names);
    }
}

impl Default for StreamingQueryExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_creation() {
        let executor = StreamingQueryExecutor::new();
        assert!(executor.engine.is_none());
    }

    #[test]
    fn test_col_names_setting() {
        let mut executor = StreamingQueryExecutor::new();
        let col_names = vec!["id".to_string(), "name".to_string()];
        executor.set_col_names(col_names.clone());
        assert_eq!(executor.col_names, Some(col_names));
    }
}
