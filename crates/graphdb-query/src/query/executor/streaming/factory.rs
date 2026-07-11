//! Streaming Executor Factory Integration
//!
//! Provides utilities to create and execute streaming queries
//! from execution plans without requiring StorageClient modifications.
//!
//! This module also handles conversion of streaming execution results
//! (Vec<DataChunk>) into the standard ExecutionResult format.

use std::sync::Arc;

use super::builder::StreamingExecutorBuilder;
use super::chunk::DataChunk;
use super::engine::StreamingExecutionEngine;
use super::executor::{SortDirection, StreamingExecutor};
use super::operators::gather_operator::GatherOperator;
use super::operators::{blocking_operator::BlockingOperator, unary_operator::UnaryOperator};
use super::partition::PartitionView;
use super::runtime::{ExecutionRuntime, QueryIdentity};
use super::stream::ResultStream;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::graph_schema::OrderDirection;
use crate::core::types::operators::AggregateFunction;
use crate::core::Value;
use crate::query::data_set::DataSet;
use crate::query::executor::base::{ExecutionContext, ExecutionResult, MemoryTracker};
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    BinaryInputNode, SingleInputNode,
};
use crate::query::planning::plan::{
    PartitionedPhysicalNode, PartitionedPhysicalPlan, PlanNodeEnum,
};

const PHYSICAL_GATHER_NODE_ID_START: i64 = i64::MIN + 100;

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
    ///
    /// # Arguments
    /// * `plan_node` - Root node of the execution plan
    /// * `context` - Execution context (for expression evaluation)
    ///
    /// # Returns
    /// * QueryError if the plan cannot be converted to streaming execution
    pub fn from_plan_node(
        &mut self,
        plan_node: &PlanNodeEnum,
        context: &ExecutionContext,
    ) -> Result<(), QueryError> {
        let executor = StreamingExecutorBuilder::from_plan_node(plan_node, context)?;

        let mut engine = StreamingExecutionEngine::new();
        engine.register_executor(0, executor);

        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: 0,
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

    /// Build a composable partitioned physical plan. Unlike the legacy
    /// root-node dispatcher, this method can place multiple global operators
    /// above local partition trees (for example `Limit(Sort(Scan))`).
    pub fn from_partitioned_physical_plan(
        &mut self,
        physical_plan: &PartitionedPhysicalPlan,
        context: &ExecutionContext,
    ) -> Result<(), QueryError> {
        let partition_view = PartitionView::try_new(
            physical_plan.partition_spec().partition_count(),
            physical_plan.partition_spec().ranges().to_vec(),
        )?;
        let mut next_gather_node_id = PHYSICAL_GATHER_NODE_ID_START;
        let root = Self::build_partitioned_physical_node(
            physical_plan.root(),
            context,
            &partition_view,
            &mut next_gather_node_id,
        )?;

        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: 0,
                session_id: None,
                space_name: context.space_name.clone(),
            },
            context.memory_budget.clone(),
        ));
        let mut engine = StreamingExecutionEngine::new();
        engine.set_runtime(runtime.clone());
        engine.register_partitioned_root(partition_view.partition_count, root)?;

        self.engine = Some(engine);
        self.runtime = Some(runtime);
        Ok(())
    }

    fn build_partitioned_physical_node(
        node: &PartitionedPhysicalNode,
        context: &ExecutionContext,
        partition_view: &PartitionView,
        next_gather_node_id: &mut i64,
    ) -> Result<StreamingExecutor, QueryError> {
        match node {
            PartitionedPhysicalNode::Local { logical_plan } => {
                let mut local_trees = StreamingExecutorBuilder::build_partitioned(
                    logical_plan,
                    context,
                    partition_view,
                )?;
                if local_trees
                    .iter()
                    .any(|executor| !executor.is_partition_local())
                {
                    return Err(QueryError::execution(format!(
                        "Physical local subtree '{}' is not partition-local",
                        logical_plan.name()
                    )));
                }
                for (partition_id, tree) in local_trees.iter_mut().enumerate() {
                    tree.set_partition_id(partition_id);
                }
                let gather_node_id = *next_gather_node_id;
                *next_gather_node_id = next_gather_node_id.checked_add(1).ok_or_else(|| {
                    QueryError::execution("Synthetic gather node id overflow".to_string())
                })?;
                Ok(StreamingExecutor::Gather(
                    OperatorBase::new(gather_node_id).with_global(true),
                    local_trees,
                    GatherOperator::concatenate(),
                ))
            }
            PartitionedPhysicalNode::GlobalUnary {
                logical_plan,
                input,
            } => {
                let input = Self::build_partitioned_physical_node(
                    input,
                    context,
                    partition_view,
                    next_gather_node_id,
                )?;
                let mut global = StreamingExecutorBuilder::from_plan_node(logical_plan, context)?;
                global.set_global();
                Self::replace_single_input(&mut global, input)?;
                Ok(global)
            }
            PartitionedPhysicalNode::GlobalBinary {
                logical_plan,
                left,
                right,
            } => {
                let left = Self::build_partitioned_physical_node(
                    left,
                    context,
                    partition_view,
                    next_gather_node_id,
                )?;
                let right = Self::build_partitioned_physical_node(
                    right,
                    context,
                    partition_view,
                    next_gather_node_id,
                )?;
                let mut global = StreamingExecutorBuilder::from_plan_node(logical_plan, context)?;
                global.set_global();
                Self::replace_binary_inputs(&mut global, left, right)?;
                Ok(global)
            }
        }
    }

    fn replace_single_input(
        root: &mut StreamingExecutor,
        input: StreamingExecutor,
    ) -> Result<(), QueryError> {
        match root {
            StreamingExecutor::Unary(_, child, _)
            | StreamingExecutor::Blocking(_, child, _)
            | StreamingExecutor::Graph(_, child, _)
            | StreamingExecutor::Sink(_, child, _)
            | StreamingExecutor::Ddl(_, child, _)
            | StreamingExecutor::Fulltext(_, child, _)
            | StreamingExecutor::Vector(_, child, _)
            | StreamingExecutor::Txn(_, child, _) => {
                **child = input;
                Ok(())
            }
            _ => Err(QueryError::execution(
                "Physical global unary node has no replaceable input".to_string(),
            )),
        }
    }

    fn replace_binary_inputs(
        root: &mut StreamingExecutor,
        left: StreamingExecutor,
        right: StreamingExecutor,
    ) -> Result<(), QueryError> {
        match root {
            StreamingExecutor::Join(_, left_child, right_child, _)
            | StreamingExecutor::Set(_, left_child, right_child, _)
            | StreamingExecutor::Apply(_, left_child, right_child, _) => {
                **left_child = left;
                **right_child = right;
                Ok(())
            }
            _ => Err(QueryError::execution(
                "Physical global binary node has no replaceable inputs".to_string(),
            )),
        }
    }

    /// Build a partitioned physical tree selected by the planner.
    ///
    /// A Sort root becomes `LocalSort × N -> Gather::MergeSort`; Aggregate,
    /// Dedup, and Limit roots become `LocalTree × N -> Gather::Concatenate
    /// -> GlobalOperator`. A local-only root is concatenated directly. Other
    /// global roots are rejected until they receive their own physical split.
    pub fn from_partitioned_plan_node(
        &mut self,
        plan_node: &PlanNodeEnum,
        context: &ExecutionContext,
        partition_view: &PartitionView,
    ) -> Result<(), QueryError> {
        if partition_view.partition_count == 0 {
            return Err(QueryError::execution(
                "Partitioned execution requires at least one partition".to_string(),
            ));
        }

        match plan_node {
            PlanNodeEnum::Sort(sort) => {
                self.from_partitioned_sort_node(sort, context, partition_view)
            }
            PlanNodeEnum::Aggregate(aggregate) => {
                self.from_partitioned_aggregate_node(aggregate, context, partition_view)
            }
            PlanNodeEnum::Dedup(dedup) => {
                self.from_partitioned_dedup_node(dedup, context, partition_view)
            }
            PlanNodeEnum::Limit(limit) => {
                self.from_partitioned_limit_node(limit, context, partition_view)
            }
            PlanNodeEnum::InnerJoin(join) => {
                self.from_partitioned_join_node(plan_node, join, context, partition_view)
            }
            PlanNodeEnum::LeftJoin(join) => {
                self.from_partitioned_join_node(plan_node, join, context, partition_view)
            }
            PlanNodeEnum::RightJoin(join) => {
                self.from_partitioned_join_node(plan_node, join, context, partition_view)
            }
            PlanNodeEnum::CrossJoin(join) => {
                self.from_partitioned_join_node(plan_node, join, context, partition_view)
            }
            PlanNodeEnum::HashInnerJoin(join) => {
                self.from_partitioned_join_node(plan_node, join, context, partition_view)
            }
            PlanNodeEnum::HashLeftJoin(join) => {
                self.from_partitioned_join_node(plan_node, join, context, partition_view)
            }
            PlanNodeEnum::FullOuterJoin(join) => {
                self.from_partitioned_join_node(plan_node, join, context, partition_view)
            }
            PlanNodeEnum::SemiJoin(join) => {
                self.from_partitioned_join_node(plan_node, join, context, partition_view)
            }
            _ => self.from_partitioned_local_plan_node(plan_node, context, partition_view),
        }
    }

    /// Build a partitioned streaming tree for a plan that consists only of
    /// partition-local operators. Results are concatenated after all local
    /// trees have been opened under the same query runtime.
    fn from_partitioned_local_plan_node(
        &mut self,
        plan_node: &PlanNodeEnum,
        context: &ExecutionContext,
        partition_view: &PartitionView,
    ) -> Result<(), QueryError> {
        let local_trees =
            StreamingExecutorBuilder::build_partitioned(plan_node, context, partition_view)?;
        if local_trees
            .iter()
            .any(|executor| !executor.is_partition_local())
        {
            return Err(QueryError::execution(
                "Plan contains global or unsupported operators and requires an explicit partitioned physical plan"
                    .to_string(),
            ));
        }

        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: 0,
                session_id: None,
                space_name: context.space_name.clone(),
            },
            context.memory_budget.clone(),
        ));
        let mut engine = StreamingExecutionEngine::new();
        engine.set_runtime(runtime.clone());
        engine.build_partitioned_executor(local_trees, GatherOperator::concatenate(), None)?;

        self.engine = Some(engine);
        self.runtime = Some(runtime);
        Ok(())
    }

    fn from_partitioned_sort_node(
        &mut self,
        sort: &crate::query::planning::plan::core::nodes::SortNode,
        context: &ExecutionContext,
        partition_view: &PartitionView,
    ) -> Result<(), QueryError> {
        let local_trees =
            StreamingExecutorBuilder::build_partitioned(sort.input(), context, partition_view)?;
        if local_trees
            .iter()
            .any(|executor| !executor.is_partition_local())
        {
            return Err(QueryError::execution(
                "Partitioned sort requires a partition-local input tree".to_string(),
            ));
        }

        let sort_expressions = sort
            .sort_items()
            .iter()
            .map(|item| item.expression.clone())
            .collect();
        let sort_directions = sort
            .sort_items()
            .iter()
            .map(|item| match item.direction {
                OrderDirection::Asc => SortDirection::Ascending,
                OrderDirection::Desc => SortDirection::Descending,
            })
            .collect();
        let limit = match sort.limit() {
            Some(value) => Some(usize::try_from(value).map_err(|_| {
                QueryError::execution("Sort limit must be non-negative".to_string())
            })?),
            None => None,
        };

        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: 0,
                session_id: None,
                space_name: context.space_name.clone(),
            },
            context.memory_budget.clone(),
        ));
        let mut engine = StreamingExecutionEngine::new();
        engine.set_runtime(runtime.clone());
        engine.build_partitioned_sort_executor(
            local_trees,
            sort_expressions,
            sort_directions,
            limit,
        )?;

        self.engine = Some(engine);
        self.runtime = Some(runtime);
        Ok(())
    }

    /// Aggregate over the concatenated local input. This deliberately does
    /// not use the Aggregate operator as both a local and final aggregate:
    /// functions such as AVG require accumulator state (sum and count), and
    /// partial results would otherwise change query semantics.
    fn from_partitioned_aggregate_node(
        &mut self,
        aggregate: &crate::query::planning::plan::core::nodes::AggregateNode,
        context: &ExecutionContext,
        partition_view: &PartitionView,
    ) -> Result<(), QueryError> {
        let local_trees = Self::build_partitioned_local_inputs(
            aggregate.input(),
            context,
            partition_view,
            "Partitioned aggregate",
        )?;
        let group_by_expressions = aggregate
            .group_keys()
            .iter()
            .map(|key| Expression::Variable(key.clone()))
            .collect();
        let aggregate_functions = Self::aggregate_functions(aggregate.aggregation_functions());
        let global = StreamingExecutor::Blocking(
            OperatorBase::new(aggregate.id()),
            Box::new(StreamingExecutor::Source(
                OperatorBase::new(i64::MIN + 2),
                super::operators::source_operator::SourceOperator::Start,
            )),
            BlockingOperator::Aggregate {
                group_by_expressions,
                aggregate_functions,
                output_col_names: aggregate.col_names().to_vec(),
                memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                state: None,
            },
        );
        self.finish_partitioned_global(local_trees, global, context)
    }

    /// Dedup must observe all partitions. Local dedup is a possible future
    /// optimization, but the global operator is the semantic boundary.
    fn from_partitioned_dedup_node(
        &mut self,
        dedup: &crate::query::planning::plan::core::nodes::DedupNode,
        context: &ExecutionContext,
        partition_view: &PartitionView,
    ) -> Result<(), QueryError> {
        let local_trees = Self::build_partitioned_local_inputs(
            dedup.input(),
            context,
            partition_view,
            "Partitioned dedup",
        )?;
        let global = StreamingExecutor::Blocking(
            OperatorBase::new(dedup.id()),
            Box::new(StreamingExecutor::Source(
                OperatorBase::new(i64::MIN + 2),
                super::operators::source_operator::SourceOperator::Start,
            )),
            BlockingOperator::Distinct {
                memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                state: None,
            },
        );
        self.finish_partitioned_global(local_trees, global, context)
    }

    /// Limit and offset are global. Applying them to every local tree would
    /// exceed the requested count and makes OFFSET incorrect across ranges.
    fn from_partitioned_limit_node(
        &mut self,
        limit: &crate::query::planning::plan::core::nodes::LimitNode,
        context: &ExecutionContext,
        partition_view: &PartitionView,
    ) -> Result<(), QueryError> {
        let count = u32::try_from(limit.count()).map_err(|_| {
            QueryError::execution("Limit count must be a non-negative u32".to_string())
        })?;
        let offset = u32::try_from(limit.offset()).map_err(|_| {
            QueryError::execution("Limit offset must be a non-negative u32".to_string())
        })?;
        let local_trees = Self::build_partitioned_local_inputs(
            limit.input(),
            context,
            partition_view,
            "Partitioned limit",
        )?;
        let global = StreamingExecutor::Unary(
            OperatorBase::new(limit.id()),
            Box::new(StreamingExecutor::Source(
                OperatorBase::new(i64::MIN + 2),
                super::operators::source_operator::SourceOperator::Start,
            )),
            UnaryOperator::Limit {
                offset,
                limit: count,
                skipped: 0,
                consumed: 0,
            },
        );
        self.finish_partitioned_global(local_trees, global, context)
    }

    /// Join both complete inputs only after their partition-local trees have
    /// been gathered. The global executor is built by the ordinary builder,
    /// preserving the existing join condition, join keys, and join variant.
    fn from_partitioned_join_node<J: BinaryInputNode>(
        &mut self,
        plan_node: &PlanNodeEnum,
        join: &J,
        context: &ExecutionContext,
        partition_view: &PartitionView,
    ) -> Result<(), QueryError> {
        let left_local_trees = Self::build_partitioned_local_inputs(
            join.left_input(),
            context,
            partition_view,
            "Partitioned join left input",
        )?;
        let right_local_trees = Self::build_partitioned_local_inputs(
            join.right_input(),
            context,
            partition_view,
            "Partitioned join right input",
        )?;
        let global_join = StreamingExecutorBuilder::from_plan_node(plan_node, context)?;
        self.finish_partitioned_join(left_local_trees, right_local_trees, global_join, context)
    }

    fn build_partitioned_local_inputs(
        input: &PlanNodeEnum,
        context: &ExecutionContext,
        partition_view: &PartitionView,
        operation: &str,
    ) -> Result<Vec<StreamingExecutor>, QueryError> {
        let local_trees =
            StreamingExecutorBuilder::build_partitioned(input, context, partition_view)?;
        if local_trees
            .iter()
            .any(|executor| !executor.is_partition_local())
        {
            return Err(QueryError::execution(format!(
                "{operation} requires a partition-local input tree"
            )));
        }
        Ok(local_trees)
    }

    fn finish_partitioned_global(
        &mut self,
        local_trees: Vec<StreamingExecutor>,
        global: StreamingExecutor,
        context: &ExecutionContext,
    ) -> Result<(), QueryError> {
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: 0,
                session_id: None,
                space_name: context.space_name.clone(),
            },
            context.memory_budget.clone(),
        ));
        let mut engine = StreamingExecutionEngine::new();
        engine.set_runtime(runtime.clone());
        engine.build_partitioned_executor(
            local_trees,
            GatherOperator::concatenate(),
            Some(global),
        )?;

        self.engine = Some(engine);
        self.runtime = Some(runtime);
        Ok(())
    }

    fn finish_partitioned_join(
        &mut self,
        left_local_trees: Vec<StreamingExecutor>,
        right_local_trees: Vec<StreamingExecutor>,
        global_join: StreamingExecutor,
        context: &ExecutionContext,
    ) -> Result<(), QueryError> {
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity {
                query_id: 0,
                session_id: None,
                space_name: context.space_name.clone(),
            },
            context.memory_budget.clone(),
        ));
        let mut engine = StreamingExecutionEngine::new();
        engine.set_runtime(runtime.clone());
        engine.build_partitioned_join_executor(left_local_trees, right_local_trees, global_join)?;

        self.engine = Some(engine);
        self.runtime = Some(runtime);
        Ok(())
    }

    fn aggregate_functions(
        functions: &[AggregateFunction],
    ) -> Vec<(AggregateFunction, Expression)> {
        functions
            .iter()
            .map(|function| {
                let expression = match function {
                    AggregateFunction::Count(Some(field))
                    | AggregateFunction::Sum(field)
                    | AggregateFunction::Avg(field)
                    | AggregateFunction::Min(field)
                    | AggregateFunction::Max(field)
                    | AggregateFunction::Collect(field) => Expression::Variable(field.clone()),
                    AggregateFunction::Count(None) => Expression::Literal(Value::Int(1)),
                    _ => Expression::Literal(Value::Int(1)),
                };
                (function.clone(), expression)
            })
            .collect()
    }

    /// Execute the streaming query
    ///
    /// # Returns
    /// * ExecutionResult with DataSet or error
    pub fn execute(&mut self) -> Result<ExecutionResult, QueryError> {
        let engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Streaming engine not initialized".to_string()))?;

        let stream = engine.into_stream()?;
        let dataset = stream.collect()?;
        Ok(ExecutionResult::DataSet(dataset))
    }

    /// Run execution through the existing materialised path (collect all chunks).
    ///
    /// Unlike `execute()` this does not require a runtime and returns `Vec<DataChunk>`
    /// for callers that need chunk-level access.
    pub fn execute_collect(&mut self) -> Result<Vec<DataChunk>, QueryError> {
        let engine = self
            .engine
            .as_mut()
            .ok_or_else(|| QueryError::execution("Streaming engine not initialized".to_string()))?;
        engine.execute()
    }

    /// Return a [`ResultStream`] for chunk-at-a-time consumption.
    ///
    /// Consumes the internal engine.  After calling this the executor
    /// can no longer be used.
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

/// Convert a Vec of DataChunks to a single DataSet
///
/// Merges all chunks into a unified result set with consistent column names
/// and aggregated rows.
///
/// # Arguments
/// * `chunks` - Vector of data chunks to merge
/// * `col_names` - Optional column names to use; if None, extracted from first chunk's schema
///
/// # Returns
/// * Result with merged DataSet or error if chunks are incompatible
pub fn convert_chunks_to_dataset(
    chunks: Vec<DataChunk>,
    col_names: Option<Vec<String>>,
) -> Result<DataSet, QueryError> {
    if chunks.is_empty() {
        let names = col_names.unwrap_or_default();
        return Ok(DataSet::with_columns(names));
    }

    // Get column names from provided arg or first chunk's schema
    let col_names = if let Some(names) = col_names {
        names
    } else {
        chunks[0].col_names()
    };

    // Validate all chunks have same column count
    let expected_cols = col_names.len();
    for (i, chunk) in chunks.iter().enumerate() {
        if chunk.num_columns() != expected_cols {
            return Err(QueryError::execution(format!(
                "Chunk {} has {} columns, expected {}",
                i,
                chunk.num_columns(),
                expected_cols
            )));
        }
    }

    // Merge all rows from all chunks
    let mut all_rows = Vec::new();
    for chunk in chunks {
        for row in chunk.rows {
            all_rows.push(row);
        }
    }

    Ok(DataSet::from_rows(all_rows, col_names))
}

/// Convert streaming execution result to ExecutionResult
///
/// # Arguments
/// * `chunks` - Result chunks from StreamingExecutionEngine
/// * `col_names` - Optional column names override
///
/// # Returns
/// * ExecutionResult with merged DataSet
pub fn chunks_to_execution_result(
    chunks: Vec<DataChunk>,
    col_names: Option<Vec<String>>,
) -> Result<ExecutionResult, QueryError> {
    let dataset = convert_chunks_to_dataset(chunks, col_names)?;
    Ok(ExecutionResult::DataSet(dataset))
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

    // Tests for convert_chunks_to_dataset and chunks_to_execution_result
    use crate::core::Value;

    fn create_test_chunk(rows: Vec<Vec<Value>>) -> DataChunk {
        DataChunk::from_rows(rows)
    }

    #[test]
    fn test_empty_chunks() {
        let result = convert_chunks_to_dataset(vec![], None);
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert!(ds.is_empty());
        assert_eq!(ds.col_count(), 0);
    }

    #[test]
    fn test_single_chunk() {
        let rows = vec![
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
        ];
        let chunk = create_test_chunk(rows);
        let result = convert_chunks_to_dataset(vec![chunk], None);
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert_eq!(ds.row_count(), 2);
        assert_eq!(ds.col_count(), 2);
    }

    #[test]
    fn test_multiple_chunks() {
        let chunk1 = create_test_chunk(vec![
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
        ]);
        let chunk2 = create_test_chunk(vec![
            vec![Value::Int(3), Value::String("c".to_string())],
            vec![Value::Int(4), Value::String("d".to_string())],
        ]);

        let result = convert_chunks_to_dataset(vec![chunk1, chunk2], None);
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert_eq!(ds.row_count(), 4);
        assert_eq!(ds.col_count(), 2);
    }

    #[test]
    fn test_execution_result_conversion() {
        let chunk = create_test_chunk(vec![vec![Value::Int(42)]]);
        let result = chunks_to_execution_result(vec![chunk], None);
        assert!(result.is_ok());
        if let ExecutionResult::DataSet(ds) = result.unwrap() {
            assert_eq!(ds.row_count(), 1);
        } else {
            panic!("Expected DataSet result");
        }
    }

    #[test]
    fn test_custom_col_names() {
        let rows = vec![vec![Value::Int(1), Value::String("test".to_string())]];
        let chunk = create_test_chunk(rows);
        let col_names = vec!["id".to_string(), "name".to_string()];
        let result = convert_chunks_to_dataset(vec![chunk], Some(col_names.clone()));
        assert!(result.is_ok());
        let ds = result.unwrap();
        assert_eq!(ds.col_names, col_names);
    }
}
