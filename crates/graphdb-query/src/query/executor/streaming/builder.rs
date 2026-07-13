use std::sync::Arc;

use super::executor::StreamingExecutor;
use super::operator_base::OperatorBase;
use super::operators::source_operator::SourceOperator;
use super::physical_node::PhysicalNode;
use super::runtime::ExecutionRuntime;
use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::{ExecutionContext, MemoryBudget};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub struct StreamingExecutorBuilder;

impl StreamingExecutorBuilder {
    /// Build an immutable PhysicalNode tree from a plan node.
    ///
    /// The returned tree can be cached and repeatedly materialized into
    /// fresh `StreamingExecutor` instances without sharing mutable state.
    pub fn from_plan_node_physical(
        node: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<PhysicalNode, QueryError> {
        super::operator_plan_builder::build_plan_node(node, context)
    }

    /// Build a `StreamingExecutor` from a PhysicalNode tree with fresh state.
    ///
    /// This is used internally by `from_plan_node` for pilot operators and
    /// can be used directly for plan caching scenarios.
    pub fn materialize_physical(
        physical: &PhysicalNode,
        runtime: Option<Arc<ExecutionRuntime>>,
        memory_budget: &MemoryBudget,
        chunk_size: usize,
    ) -> StreamingExecutor {
        physical.materialize(runtime, memory_budget, chunk_size)
    }

    pub fn build_simple_scan(rows: Vec<Vec<Value>>) -> Result<StreamingExecutor, QueryError> {
        Self::build_simple_scan_with_col_names(rows, vec![])
    }

    pub fn build_simple_scan_with_col_names(
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
    ) -> Result<StreamingExecutor, QueryError> {
        Ok(StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                partition_id: 0,
                buffer: rows,
                current_index: 0,
                col_names,
            },
        ))
    }

    pub fn from_plan_node(
        node: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<StreamingExecutor, QueryError> {
        let physical = Self::from_plan_node_physical(node, context)?;
        let execution_runtime = ExecutionRuntime::new(
            crate::query::executor::streaming::runtime::QueryIdentity {
                query_id: context.query_id,
                session_id: None,
                space_name: context.space_name.clone(),
            },
            context.memory_budget.clone(),
            context.storage.clone(),
            #[cfg(feature = "fulltext-search")]
            context.fulltext_manager.clone(),
            #[cfg(feature = "qdrant")]
            context.vector_coordinator.clone(),
        );
        let runtime = Arc::new(execution_runtime);
        let executor = Self::materialize_physical(
            &physical,
            Some(runtime),
            &context.memory_budget,
            context.chunk_size,
        );
        Ok(executor)
    }

    pub fn from_plan(
        plan: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<StreamingExecutor, QueryError> {
        let mut executor = Self::from_plan_node(plan, context)?;
        executor.set_chunk_size(context.chunk_size);
        Ok(executor)
    }

    /// Build a simple scan with partition info for testing.
    pub fn build_simple_scan_partitioned(
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
        partition_id: usize,
    ) -> Result<StreamingExecutor, QueryError> {
        Ok(StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                partition_id,
                buffer: rows,
                current_index: 0,
                col_names,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_scan_creation() {
        let rows = vec![
            vec![Value::BigInt(1), Value::String("a".to_string())],
            vec![Value::BigInt(2), Value::String("b".to_string())],
        ];
        let executor = StreamingExecutorBuilder::build_simple_scan(rows.clone()).unwrap();
        match executor {
            StreamingExecutor::Source(_, SourceOperator::ScanVertices { buffer, .. }) => {
                assert_eq!(buffer.len(), 2);
            }
            _ => panic!("Expected ScanVertices"),
        }
    }

    #[test]
    fn test_empty_scan_creation() {
        let rows: Vec<Vec<Value>> = vec![];
        let executor = StreamingExecutorBuilder::build_simple_scan(rows).unwrap();
        match executor {
            StreamingExecutor::Source(_, SourceOperator::ScanVertices { buffer, .. }) => {
                assert!(buffer.is_empty());
            }
            _ => panic!("Expected ScanVertices"),
        }
    }

    #[test]
    fn unsupported_control_flow_node_fails_during_build() {
        use crate::query::planning::plan::core::nodes::PassThroughNode;

        let node = PlanNodeEnum::PassThrough(PassThroughNode::new(42));
        let error = StreamingExecutorBuilder::from_plan_node(&node, &ExecutionContext::default())
            .expect_err("unsupported control flow must not silently pass through");

        assert!(error.to_string().contains("PassThrough"));
    }
}
