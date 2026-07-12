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
use super::executor::StreamingExecutor;
use super::operator_base::OperatorBase;
use super::operators::blocking_operator::BlockingOperator;
use super::operators::gather_operator::GatherOperator;
use super::partition::PartitionView;
use super::runtime::{ExecutionRuntime, QueryIdentity};
use super::stream::ResultStream;
use super::stream_result::StreamingQueryResult;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::types::operators::BinaryOperator;
use crate::query::data_set::DataSet;
use crate::query::executor::base::{ExecutionContext, ExecutionResult, MemoryTracker};
use crate::query::planning::plan::{
    PartitionedPhysicalNode, PartitionedPhysicalPlan, PlanNodeEnum,
};

use super::operators::shuffle_join_operator::{HashJoinKind, HashShuffleJoinOperator};

const PHYSICAL_GATHER_NODE_ID_START: i64 = i64::MIN + 100;

/// Intermediate build result for the factory's recursive construction.
/// - `Global`: a single executor tree (result of a global or exchange operator).
/// - `Local`: a set of per-partition trees that have not yet been gathered.
enum BuildOutput {
    Global(StreamingExecutor),
    Local(Vec<StreamingExecutor>),
}

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
        // max_workers defaults to 1 and is NOT wired for parallelism in
        // production.  The coordinator always falls back to sequential.
        engine.set_max_workers(context.max_workers);
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

    /// Build a composable partitioned physical plan. Unlike the legacy
    /// root-node dispatcher, this method can place multiple global operators
    /// above local partition trees (for example `Limit(Sort(Scan))`).
    pub fn from_partitioned_physical_plan(
        &mut self,
        physical_plan: &PartitionedPhysicalPlan,
        context: &ExecutionContext,
    ) -> Result<(), QueryError> {
        let partition_view = PartitionView::from(physical_plan.partition_spec());
        let mut next_gather_node_id = PHYSICAL_GATHER_NODE_ID_START;
        let root = Self::build_partitioned_physical_node(
            physical_plan.root(),
            context,
            &partition_view,
            &mut next_gather_node_id,
        )?;

        let root = match root {
            BuildOutput::Global(executor) => executor,
            BuildOutput::Local(trees) => {
                Self::local_to_global(BuildOutput::Local(trees), &mut next_gather_node_id)?
            }
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
        // max_workers defaults to 1; parallel coordinator is experimental.
        engine.set_max_workers(context.max_workers);
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
    ) -> Result<BuildOutput, QueryError> {
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
                Ok(BuildOutput::Local(local_trees))
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
                let input = Self::local_to_global(input, next_gather_node_id)?;
                let mut global = StreamingExecutorBuilder::from_plan_node(logical_plan, context)?;
                global.set_global();
                Self::replace_single_input(&mut global, input)?;
                Ok(BuildOutput::Global(global))
            }
            PartitionedPhysicalNode::AggregateSplit {
                logical_plan,
                input,
            } => {
                let aggregate = match logical_plan {
                    PlanNodeEnum::Aggregate(a) => a,
                    _ => {
                        return Err(QueryError::execution(
                            "AggregateSplit requires an Aggregate logical plan".to_string(),
                        ));
                    }
                };
                let local_plan = match input.as_ref() {
                    PartitionedPhysicalNode::Local { logical_plan } => logical_plan,
                    _ => {
                        return Err(QueryError::execution(
                            "AggregateSplit input must be a Local node".to_string(),
                        ));
                    }
                };
                let local_trees = StreamingExecutorBuilder::build_partitioned(
                    local_plan,
                    context,
                    partition_view,
                )?;
                for executor in &local_trees {
                    if !executor.is_partition_local() {
                        return Err(QueryError::execution(format!(
                            "AggregateSplit local subtree '{}' is not partition-local",
                            local_plan.name()
                        )));
                    }
                }

                let group_by_expressions: Vec<Expression> = aggregate
                    .group_keys()
                    .iter()
                    .map(|key| Expression::Variable(key.clone()))
                    .collect();
                let aggregate_functions: Vec<AggregateFunction> =
                    aggregate.aggregation_functions().to_vec();
                let output_col_names = aggregate.col_names().to_vec();

                let partial_aggregates: Vec<StreamingExecutor> = local_trees
                    .into_iter()
                    .map(|tree| {
                        StreamingExecutor::Blocking(
                            OperatorBase::new(aggregate.id()),
                            Box::new(tree),
                            BlockingOperator::PartialAggregate {
                                group_by_expressions: group_by_expressions.clone(),
                                aggregate_functions: aggregate_functions.clone(),
                                output_col_names: output_col_names.clone(),
                                memory_tracker: MemoryTracker::new(
                                    context.memory_budget.clone(),
                                ),
                                state: None,
                            },
                        )
                    })
                    .collect();

                let gather_node_id = *next_gather_node_id;
                *next_gather_node_id = next_gather_node_id.checked_add(1).ok_or_else(|| {
                    QueryError::execution("Synthetic gather node id overflow".to_string())
                })?;
                let gather = StreamingExecutor::Gather(
                    OperatorBase::new(gather_node_id).with_global(true),
                    partial_aggregates,
                    GatherOperator::concatenate(),
                );

                let mut final_aggregate = StreamingExecutor::Blocking(
                    OperatorBase::new(aggregate.id()).with_global(true),
                    Box::new(StreamingExecutor::Source(
                        OperatorBase::new(i64::MIN + 2),
                        super::operators::source_operator::SourceOperator::Start,
                    )),
                    BlockingOperator::FinalAggregate {
                        group_by_expressions,
                        aggregate_functions,
                        output_col_names,
                        memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                        state: None,
                    },
                );
                Self::replace_single_input(&mut final_aggregate, gather)?;
                Ok(BuildOutput::Global(final_aggregate))
            }
            PartitionedPhysicalNode::DistinctSplit {
                logical_plan,
                input,
            } => {
                let input_node = match input.as_ref() {
                    PartitionedPhysicalNode::Local { logical_plan } => logical_plan,
                    _ => {
                        return Err(QueryError::execution(
                            "DistinctSplit input must be a Local node".to_string(),
                        ));
                    }
                };
                let local_trees = StreamingExecutorBuilder::build_partitioned(
                    input_node,
                    context,
                    partition_view,
                )?;
                for executor in &local_trees {
                    if !executor.is_partition_local() {
                        return Err(QueryError::execution(
                            "DistinctSplit local subtree is not partition-local".to_string(),
                        ));
                    }
                }

                let memory_tracker = MemoryTracker::new(context.memory_budget.clone());
                let local_distincts: Vec<StreamingExecutor> = local_trees
                    .into_iter()
                    .map(|tree| {
                        StreamingExecutor::Blocking(
                            OperatorBase::new(logical_plan.id()),
                            Box::new(tree),
                            BlockingOperator::Distinct {
                                memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                                state: None,
                            },
                        )
                    })
                    .collect();

                let gather_node_id = *next_gather_node_id;
                *next_gather_node_id = next_gather_node_id.checked_add(1).ok_or_else(|| {
                    QueryError::execution("Synthetic gather node id overflow".to_string())
                })?;
                let gather = StreamingExecutor::Gather(
                    OperatorBase::new(gather_node_id).with_global(true),
                    local_distincts,
                    GatherOperator::concatenate(),
                );

                let mut global_distinct = StreamingExecutor::Blocking(
                    OperatorBase::new(logical_plan.id()).with_global(true),
                    Box::new(StreamingExecutor::Source(
                        OperatorBase::new(i64::MIN + 2),
                        super::operators::source_operator::SourceOperator::Start,
                    )),
                    BlockingOperator::Distinct {
                        memory_tracker,
                        state: None,
                    },
                );
                Self::replace_single_input(&mut global_distinct, gather)?;
                Ok(BuildOutput::Global(global_distinct))
            }
            PartitionedPhysicalNode::TopNSplit {
                logical_plan,
                input,
            } => {
                let topn_node = match logical_plan {
                    PlanNodeEnum::TopN(t) => t,
                    _ => {
                        return Err(QueryError::execution(
                            "TopNSplit requires a TopN logical plan".to_string(),
                        ));
                    }
                };
                let input_node = match input.as_ref() {
                    PartitionedPhysicalNode::Local { logical_plan } => logical_plan,
                    _ => {
                        return Err(QueryError::execution(
                            "TopNSplit input must be a Local node".to_string(),
                        ));
                    }
                };
                let local_trees = StreamingExecutorBuilder::build_partitioned(
                    input_node,
                    context,
                    partition_view,
                )?;
                for executor in &local_trees {
                    if !executor.is_partition_local() {
                        return Err(QueryError::execution(
                            "TopNSplit local subtree is not partition-local".to_string(),
                        ));
                    }
                }

                let limit = topn_node.limit() as u32;
                let sort_items = topn_node.sort_items();
                let (sort_expressions, sort_directions) =
                    StreamingExecutorBuilder::sort_items_to_expressions(sort_items)
                        .map_err(|e| QueryError::execution(format!("TopNSplit sort error: {e}")))?;

                let local_topns: Vec<StreamingExecutor> = local_trees
                    .into_iter()
                    .map(|tree| {
                        StreamingExecutor::Blocking(
                            OperatorBase::new(logical_plan.id()),
                            Box::new(tree),
                            BlockingOperator::TopN {
                                n: limit,
                                sort_expressions: sort_expressions.clone(),
                                sort_directions: sort_directions.clone(),
                                memory_tracker: MemoryTracker::new(
                                    context.memory_budget.clone(),
                                ),
                                state: None,
                            },
                        )
                    })
                    .collect();

                let gather_node_id = *next_gather_node_id;
                *next_gather_node_id = next_gather_node_id.checked_add(1).ok_or_else(|| {
                    QueryError::execution("Synthetic gather node id overflow".to_string())
                })?;
                Ok(BuildOutput::Global(StreamingExecutor::Gather(
                    OperatorBase::new(gather_node_id).with_global(true),
                    local_topns,
                    GatherOperator::merge_sort(
                        sort_expressions,
                        sort_directions,
                        Some(limit as usize),
                    ),
                )))
            }
            PartitionedPhysicalNode::HashJoinExchange {
                logical_plan,
                left,
                right,
                bucket_count,
            } => {
                let (left_input, right_input) = match (left.as_ref(), right.as_ref()) {
                    (PartitionedPhysicalNode::Local { .. }, PartitionedPhysicalNode::Local { .. }) => {
                        let left_output = Self::build_partitioned_physical_node(
                            left,
                            context,
                            partition_view,
                            next_gather_node_id,
                        )?;
                        let right_output = Self::build_partitioned_physical_node(
                            right,
                            context,
                            partition_view,
                            next_gather_node_id,
                        )?;
                        match (left_output, right_output) {
                            (BuildOutput::Local(left_trees), BuildOutput::Local(right_trees)) => {
                                (left_trees, right_trees)
                            }
                            _ => {
                                return Err(QueryError::execution(
                                    "HashJoinExchange requires Local inputs".to_string(),
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(QueryError::execution(
                            "HashJoinExchange requires Local children in PartitionedPhysicalPlan"
                                .to_string(),
                        ));
                    }
                };

                let (left_key_exprs, right_key_exprs, join_condition, join_kind, left_schema, right_schema) = match logical_plan {
                    PlanNodeEnum::HashInnerJoin(join_node) => {
                        let hash_keys = Self::join_keys_to_expressions(join_node.hash_keys())?;
                        let probe_keys = Self::join_keys_to_expressions(join_node.probe_keys())?;
                        let condition = Self::join_keys_to_condition(
                            join_node.hash_keys(),
                            join_node.probe_keys(),
                            join_node.right_input().col_names(),
                        )?;
                        let left_schema = join_node.left_input().col_names().to_vec();
                        let right_schema = join_node.right_input().col_names().to_vec();
                        // HashJoin uses left as probe, right as build.
                        // probe_keys are evaluated on left, hash_keys on right.
                        (probe_keys, hash_keys, condition, HashJoinKind::Inner, left_schema, right_schema)
                    }
                    PlanNodeEnum::HashLeftJoin(join_node) => {
                        let hash_keys = Self::join_keys_to_expressions(join_node.hash_keys())?;
                        let probe_keys = Self::join_keys_to_expressions(join_node.probe_keys())?;
                        let condition = Self::join_keys_to_condition(
                            join_node.hash_keys(),
                            join_node.probe_keys(),
                            join_node.right_input().col_names(),
                        )?;
                        let left_schema = join_node.left_input().col_names().to_vec();
                        let right_schema = join_node.right_input().col_names().to_vec();
                        (probe_keys, hash_keys, condition, HashJoinKind::Left, left_schema, right_schema)
                    }
                    _ => {
                        return Err(QueryError::execution(
                            "HashJoinExchange requires a HashInnerJoin or HashLeftJoin logical plan"
                                .to_string(),
                        ));
                    }
                };

                let memory_tracker = MemoryTracker::new(context.memory_budget.clone());
                let operator = HashShuffleJoinOperator::new(
                    join_kind,
                    left_key_exprs,
                    right_key_exprs,
                    join_condition,
                    *bucket_count,
                    left_schema,
                    right_schema,
                    memory_tracker,
                );

                let join_executor = StreamingExecutor::HashShuffleJoin(
                    OperatorBase::new(logical_plan.id()).with_global(true),
                    left_input,
                    right_input,
                    operator,
                );
                Ok(BuildOutput::Global(join_executor))
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
                let left = Self::local_to_global(left, next_gather_node_id)?;
                let right = Self::build_partitioned_physical_node(
                    right,
                    context,
                    partition_view,
                    next_gather_node_id,
                )?;
                let right = Self::local_to_global(right, next_gather_node_id)?;
                let mut global = StreamingExecutorBuilder::from_plan_node(logical_plan, context)?;
                global.set_global();
                Self::replace_binary_inputs(&mut global, left, right)?;
                Ok(BuildOutput::Global(global))
            }
        }
    }

    /// Convert a BuildOutput::Local to a global executor by wrapping with
    /// Gather::Concatenate. Identity for BuildOutput::Global.
    fn local_to_global(
        output: BuildOutput,
        next_gather_node_id: &mut i64,
    ) -> Result<StreamingExecutor, QueryError> {
        match output {
            BuildOutput::Global(executor) => Ok(executor),
            BuildOutput::Local(trees) => {
                if trees.is_empty() {
                    return Err(QueryError::execution(
                        "Cannot gather empty local trees".to_string(),
                    ));
                }
                let gather_node_id = *next_gather_node_id;
                *next_gather_node_id = next_gather_node_id.checked_add(1).ok_or_else(|| {
                    QueryError::execution("Synthetic gather node id overflow".to_string())
                })?;
                Ok(StreamingExecutor::Gather(
                    OperatorBase::new(gather_node_id).with_global(true),
                    trees,
                    GatherOperator::concatenate(),
                ))
            }
        }
    }

    fn join_keys_to_expressions(
        keys: &[crate::core::types::expr::ContextualExpression],
    ) -> Result<Vec<Expression>, QueryError> {
        keys.iter()
            .map(|k| {
                k.get_expression().ok_or_else(|| {
                    QueryError::execution("Failed to resolve join key expression".to_string())
                })
            })
            .collect()
    }

    fn join_keys_to_condition(
        hash_keys: &[crate::core::types::expr::ContextualExpression],
        probe_keys: &[crate::core::types::expr::ContextualExpression],
        _right_col_names: &[String],
    ) -> Result<Option<Expression>, QueryError> {
        if hash_keys.is_empty() || probe_keys.is_empty() || hash_keys.len() != probe_keys.len() {
            return Ok(None);
        }
        let left_first = hash_keys[0].get_expression().ok_or_else(|| {
            QueryError::execution("Failed to resolve hash key expression".to_string())
        })?;
        let right_first = probe_keys[0].get_expression().ok_or_else(|| {
            QueryError::execution("Failed to resolve probe key expression".to_string())
        })?;
        let mut condition = Expression::Binary {
            left: Box::new(left_first),
            op: BinaryOperator::Equal,
            right: Box::new(right_first),
        };
        for i in 1..hash_keys.len() {
            let left = hash_keys[i].get_expression().ok_or_else(|| {
                QueryError::execution("Failed to resolve hash key expression".to_string())
            })?;
            let right = probe_keys[i].get_expression().ok_or_else(|| {
                QueryError::execution("Failed to resolve probe key expression".to_string())
            })?;
            let eq = Expression::Binary {
                left: Box::new(left),
                op: BinaryOperator::Equal,
                right: Box::new(right),
            };
            condition = Expression::Binary {
                left: Box::new(condition),
                op: BinaryOperator::And,
                right: Box::new(eq),
            };
        }
        Ok(Some(condition))
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

    /// Execute the query and materialize all results into a DataSet.
    ///
    /// # Returns
    /// * ExecutionResult::DataSet with all rows, or error
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
    ///
    /// Consumes the internal engine.  After calling this the executor
    /// can no longer be used.
    pub fn execute_to_stream(&mut self) -> Result<StreamingQueryResult, QueryError> {
        let engine = self
            .engine
            .take()
            .ok_or_else(|| QueryError::execution("Streaming engine not initialized".to_string()))?;
        let runtime = self
            .runtime
            .take()
            .ok_or_else(|| QueryError::execution("Streaming runtime not initialized".to_string()))?;
        let stream = engine.into_stream()?;
        Ok(StreamingQueryResult::new(stream, runtime))
    }

    /// Run execution through the existing materialised path (collect all chunks).
    ///
    /// Unlike `execute_materialized()` this does not require a runtime and returns `Vec<DataChunk>`
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
    Ok(ExecutionResult::DataSet {
        data: dataset,
        execution_mode_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::base::MemoryBudget;
    use crate::query::executor::streaming::executor::StreamingExecutor;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::planning::plan::PartitionSpec;

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
    use crate::query::executor::streaming::executor::SortDirection;

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
        if let ExecutionResult::DataSet { data: ds, .. } = result.unwrap() {
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

    // ── Partial + Final Aggregate integration tests ──

    fn scan_executor(rows: Vec<Vec<Value>>, partition_id: usize, col_names: Vec<String>) -> StreamingExecutor {
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                partition_id,
                buffer: rows,
                current_index: 0,
                col_names,
            },
        )
    }

    fn collect_ids(chunks: Vec<DataChunk>) -> Vec<i64> {
        let mut ids: Vec<i64> = chunks
            .iter()
            .flat_map(|c| c.rows.iter())
            .filter_map(|row| row.first())
            .filter_map(|v| match v {
                Value::BigInt(n) => Some(*n),
                Value::Int(n) => Some(*n as i64),
                _ => None,
            })
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn test_partial_aggregate_operator_counts_rows_per_partition() {
        let mut partial = StreamingExecutor::Blocking(
            OperatorBase::new(1),
            Box::new(scan_executor(
                vec![
                    vec![Value::BigInt(1), Value::BigInt(10)],
                    vec![Value::BigInt(2), Value::BigInt(20)],
                ],
                0,
                vec!["id".to_string(), "amount".to_string()],
            )),
            BlockingOperator::PartialAggregate {
                group_by_expressions: Vec::new(),
                aggregate_functions: vec![
                    AggregateFunction::Count(None),
                    AggregateFunction::Sum("amount".to_string()),
                ],
                output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
        );
        partial.open().expect("partial open");
        let chunk = partial.advance().expect("partial advance");
        partial.close().expect("partial close");

        let chunk = chunk.expect("partial should produce a chunk");
        assert_eq!(chunk.rows.len(), 1, "should have one group row");
        // Partial output: first acc is Count(2), second is Sum(30)
        assert_eq!(chunk.rows[0].len(), 2);
        // Count accumulator
        match &chunk.rows[0][0] {
            Value::BigInt(n) => assert_eq!(*n, 2),
            _ => panic!("expected BigInt for Count"),
        }
        // Sum accumulator
        match &chunk.rows[0][1] {
            Value::Double(n) => assert!((*n - 30.0).abs() < 1e-10),
            _ => panic!("expected Double for Sum"),
        }
    }

    #[test]
    fn test_final_aggregate_merges_partial_results() {
        // Simulate two partitions each producing a partial result
        let p0 = StreamingExecutor::Blocking(
            OperatorBase::new(1),
            Box::new(scan_executor(
                vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
                0,
                vec!["val".to_string()],
            )),
            BlockingOperator::PartialAggregate {
                group_by_expressions: Vec::new(),
                aggregate_functions: vec![
                    AggregateFunction::Count(None),
                    AggregateFunction::Sum("val".to_string()),
                ],
                output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
        );
        let p1 = StreamingExecutor::Blocking(
            OperatorBase::new(1),
            Box::new(scan_executor(
                vec![vec![Value::BigInt(3)], vec![Value::BigInt(4)]],
                1,
                vec!["val".to_string()],
            )),
            BlockingOperator::PartialAggregate {
                group_by_expressions: Vec::new(),
                aggregate_functions: vec![
                    AggregateFunction::Count(None),
                    AggregateFunction::Sum("val".to_string()),
                ],
                output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
        );

        let mut final_agg = StreamingExecutor::Blocking(
            OperatorBase::new(3).with_global(true),
            Box::new(StreamingExecutor::Gather(
                OperatorBase::new(2).with_global(true),
                vec![p0, p1],
                GatherOperator::concatenate(),
            )),
            BlockingOperator::FinalAggregate {
                group_by_expressions: Vec::new(),
                aggregate_functions: vec![
                    AggregateFunction::Count(None),
                    AggregateFunction::Sum("val".to_string()),
                ],
                output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
        );

        final_agg.open().expect("final open");
        let chunk = final_agg.advance().expect("final advance");
        final_agg.close().expect("final close");

        let chunk = chunk.expect("final should produce a chunk");
        assert_eq!(chunk.rows.len(), 1, "should have one result row");
        assert_eq!(chunk.rows[0][0], Value::BigInt(4), "COUNT should be 4");
        match &chunk.rows[0][1] {
            Value::Double(n) => assert!((*n - 10.0).abs() < 1e-10, "SUM should be ~10"),
            Value::BigInt(n) => assert_eq!(*n, 10, "SUM should be 10"),
            other => panic!("expected numeric for SUM, got {:?}", other),
        }
    }

    #[test]
    fn test_partial_aggregate_with_group_keys() {
        let mut partial = StreamingExecutor::Blocking(
            OperatorBase::new(1),
            Box::new(scan_executor(
                vec![
                    vec![Value::String("a".to_string()), Value::BigInt(10)],
                    vec![Value::String("a".to_string()), Value::BigInt(20)],
                    vec![Value::String("b".to_string()), Value::BigInt(30)],
                ],
                0,
                vec!["group".to_string(), "amount".to_string()],
            )),
            BlockingOperator::PartialAggregate {
                group_by_expressions: vec![Expression::Variable("group".to_string())],
                aggregate_functions: vec![
                    AggregateFunction::Count(None),
                    AggregateFunction::Sum("amount".to_string()),
                ],
                output_col_names: vec!["group".to_string(), "COUNT".to_string(), "SUM".to_string()],
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
        );
        partial.open().expect("partial open");
        let chunk = partial.advance().expect("partial advance");
        partial.close().expect("partial close");

        let chunk = chunk.expect("partial should produce rows");
        assert_eq!(chunk.rows.len(), 2, "should have two group rows");
        // HashMap traversal is non-deterministic, so check by grouping
        let mut by_group: std::collections::HashMap<String, &Vec<Value>> = std::collections::HashMap::new();
        for row in &chunk.rows {
            let key = match &row[0] {
                Value::String(s) => s.clone(),
                _ => panic!("expected String group key"),
            };
            by_group.insert(key, row);
        }
        let row_a = by_group.get("a").expect("group 'a' should exist");
        let row_b = by_group.get("b").expect("group 'b' should exist");
        assert_eq!(row_a[1], Value::BigInt(2), "group a COUNT should be 2");
        assert_eq!(row_b[1], Value::BigInt(1), "group b COUNT should be 1");
    }

    #[test]
    fn test_aggregate_split_via_partitioned_physical_plan() {
        // Build a PartitionedPhysicalPlan with AggregateSplit using from_logical
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
        use crate::query::planning::plan::PartitionSource;

        let start = StartNode::new();
        let agg = AggregateNode::new(
            PlanNodeEnum::Start(start),
            vec![],
            vec![
                AggregateFunction::Count(None),
                AggregateFunction::Sum("amount".to_string()),
            ],
        )
        .expect("aggregate plan should build");
        let spec = PartitionSpec::try_new(
            vec![0..10, 10..20],
            PartitionSource::VertexId { tag: "test".to_string() },
            None,
        )
        .expect("valid spec");
        let physical = PartitionedPhysicalPlan::from_logical(
            PlanNodeEnum::Aggregate(agg),
            spec,
        );

        // Verify we get AggregateSplit for supported functions
        assert!(
            matches!(physical.root(), PartitionedPhysicalNode::AggregateSplit { .. }),
            "Expected AggregateSplit, got {:?}",
            physical.root()
        );
    }

    #[test]
    fn test_two_level_distinct_eliminates_duplicates_across_partitions() {
        // Partition 0: [1, 1, 2], Partition 1: [2, 3, 3]
        let p0 = scan_executor(
            vec![
                vec![Value::BigInt(1)],
                vec![Value::BigInt(1)],
                vec![Value::BigInt(2)],
            ],
            0,
            vec!["val".to_string()],
        );
        let p1 = scan_executor(
            vec![
                vec![Value::BigInt(2)],
                vec![Value::BigInt(3)],
                vec![Value::BigInt(3)],
            ],
            1,
            vec!["val".to_string()],
        );

        let local_distincts: Vec<StreamingExecutor> = vec![p0, p1]
            .into_iter()
            .map(|tree| {
                StreamingExecutor::Blocking(
                    OperatorBase::new(1),
                    Box::new(tree),
                    BlockingOperator::Distinct {
                        memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                        state: None,
                    },
                )
            })
            .collect();

        let mut global_distinct = StreamingExecutor::Blocking(
            OperatorBase::new(3).with_global(true),
            Box::new(StreamingExecutor::Gather(
                OperatorBase::new(2).with_global(true),
                local_distincts,
                GatherOperator::concatenate(),
            )),
            BlockingOperator::Distinct {
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
        );

        global_distinct.open().expect("distinct pipeline open");
        let chunk = global_distinct.advance().expect("distinct advance");
        global_distinct.close().expect("distinct close");

        let chunk = chunk.expect("distinct should produce rows");
        let mut values: Vec<i64> = chunk
            .rows
            .iter()
            .filter_map(|row| match row.first() {
                Some(Value::BigInt(n)) => Some(*n),
                _ => None,
            })
            .collect();
        values.sort();
        assert_eq!(values, vec![1, 2, 3], "two-level distinct should produce [1, 2, 3]");
    }

    #[test]
    fn test_two_level_topn_keeps_top_across_partitions() {
        // Partition 0: [5, 3, 1], Partition 1: [4, 2, 6]
        // Each local TopN(3) keeps all; MergeSort(limit=3) returns [1, 2, 3]
        let p0 = scan_executor(
            vec![
                vec![Value::BigInt(5)],
                vec![Value::BigInt(3)],
                vec![Value::BigInt(1)],
            ],
            0,
            vec!["val".to_string()],
        );
        let p1 = scan_executor(
            vec![
                vec![Value::BigInt(4)],
                vec![Value::BigInt(2)],
                vec![Value::BigInt(6)],
            ],
            1,
            vec!["val".to_string()],
        );

        let limit: u32 = 3;
        let sort_expressions = vec![Expression::Variable("val".to_string())];
        let sort_directions = vec![SortDirection::Ascending];

        let local_topns: Vec<StreamingExecutor> = vec![p0, p1]
            .into_iter()
            .map(|tree| {
                StreamingExecutor::Blocking(
                    OperatorBase::new(1),
                    Box::new(tree),
                    BlockingOperator::TopN {
                        n: limit,
                        sort_expressions: sort_expressions.clone(),
                        sort_directions: sort_directions.clone(),
                        memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                        state: None,
                    },
                )
            })
            .collect();

        let mut executor = StreamingExecutor::Gather(
            OperatorBase::new(2).with_global(true),
            local_topns,
            GatherOperator::merge_sort(
                sort_expressions,
                sort_directions,
                Some(limit as usize),
            ),
        );

        executor.open().expect("topn pipeline open");
        let chunk = executor.advance().expect("topn advance");
        executor.close().expect("topn close");

        let chunk = chunk.expect("topn should produce rows");
        let values: Vec<i64> = chunk
            .rows
            .iter()
            .filter_map(|row| match row.first() {
                Some(Value::BigInt(n)) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(values, vec![1, 2, 3], "two-level TopN(3) should produce [1, 2, 3]");
    }

    // ── HashShuffleJoin integration tests ──

    fn make_hash_shuffle_join(
        left_trees: Vec<StreamingExecutor>,
        right_trees: Vec<StreamingExecutor>,
        join_kind: HashJoinKind,
        left_key: &str,
        right_key: &str,
        left_schema: Vec<String>,
        right_schema: Vec<String>,
        bucket_count: usize,
    ) -> StreamingExecutor {
        let left_key_expr = if left_key.is_empty() {
            vec![]
        } else {
            vec![Expression::Variable(left_key.to_string())]
        };
        let right_key_expr = if right_key.is_empty() {
            vec![]
        } else {
            vec![Expression::Variable(right_key.to_string())]
        };
        let operator = HashShuffleJoinOperator::new(
            join_kind,
            left_key_expr,
            right_key_expr,
            None,
            bucket_count,
            left_schema,
            right_schema,
            MemoryTracker::new(MemoryBudget::default_budget()),
        );
        let node_id = 100;
        StreamingExecutor::HashShuffleJoin(
            OperatorBase::new(node_id).with_global(true),
            left_trees,
            right_trees,
            operator,
        )
    }

    #[test]
    fn test_hash_shuffle_inner_join_matches_across_partitions() {
        let p0 = scan_executor(
            vec![
                vec![Value::BigInt(1), Value::String("a".to_string())],
                vec![Value::BigInt(2), Value::String("b".to_string())],
            ],
            0,
            vec!["id".to_string(), "val".to_string()],
        );
        let p1 = scan_executor(
            vec![
                vec![Value::BigInt(3), Value::String("c".to_string())],
            ],
            1,
            vec!["id".to_string(), "val".to_string()],
        );

        let r0 = scan_executor(
            vec![
                vec![Value::BigInt(1), Value::BigInt(100)],
                vec![Value::BigInt(3), Value::BigInt(300)],
            ],
            0,
            vec!["id".to_string(), "score".to_string()],
        );
        let r1 = scan_executor(
            vec![
                vec![Value::BigInt(2), Value::BigInt(200)],
            ],
            1,
            vec!["id".to_string(), "score".to_string()],
        );

        let mut join = make_hash_shuffle_join(
            vec![p0, p1],
            vec![r0, r1],
            HashJoinKind::Inner,
            "id", "id",
            vec!["id".to_string(), "val".to_string()],
            vec!["id".to_string(), "score".to_string()],
            4,
        );

        fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
            let mut all_rows = Vec::new();
            while let Ok(Some(chunk)) = join.advance() {
                all_rows.extend(chunk.rows);
            }
            all_rows
        }

        join.open().expect("hash shuffle inner join open");
        let mut rows = collect_all(&mut join);
        join.close().expect("hash shuffle inner join close");

        rows.sort_by(|a, b| {
            let a_id = match a.first() { Some(Value::BigInt(n)) => *n, _ => 0 };
            let b_id = match b.first() { Some(Value::BigInt(n)) => *n, _ => 0 };
            a_id.cmp(&b_id)
        });
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec![Value::BigInt(1), Value::String("a".to_string()), Value::BigInt(1), Value::BigInt(100)]);
        assert_eq!(rows[1], vec![Value::BigInt(2), Value::String("b".to_string()), Value::BigInt(2), Value::BigInt(200)]);
        assert_eq!(rows[2], vec![Value::BigInt(3), Value::String("c".to_string()), Value::BigInt(3), Value::BigInt(300)]);
    }

    #[test]
    fn test_hash_shuffle_left_join_pads_nulls_for_unmatched() {
        fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
            let mut all_rows = Vec::new();
            while let Ok(Some(chunk)) = join.advance() {
                all_rows.extend(chunk.rows);
            }
            all_rows
        }
        let p0 = scan_executor(
            vec![
                vec![Value::BigInt(1)],
                vec![Value::BigInt(2)],
            ],
            0,
            vec!["id".to_string()],
        );
        let p1 = scan_executor(
            vec![vec![Value::BigInt(3)]],
            1,
            vec!["id".to_string()],
        );

        let r0 = scan_executor(
            vec![vec![Value::BigInt(1), Value::BigInt(100)]],
            0,
            vec!["id".to_string(), "score".to_string()],
        );
        let r1 = scan_executor(
            vec![],
            1,
            vec!["id".to_string(), "score".to_string()],
        );

        let mut join = make_hash_shuffle_join(
            vec![p0, p1],
            vec![r0, r1],
            HashJoinKind::Left,
            "id", "id",
            vec!["id".to_string()],
            vec!["id".to_string(), "score".to_string()],
            4,
        );

        join.open().expect("hash shuffle left join open");
        let mut rows = collect_all(&mut join);
        join.close().expect("hash shuffle left join close");

        rows.sort_by(|a, b| {
            let a_id = match a.first() { Some(Value::BigInt(n)) => *n, _ => 0 };
            let b_id = match b.first() { Some(Value::BigInt(n)) => *n, _ => 0 };
            a_id.cmp(&b_id)
        });
        assert_eq!(rows.len(), 3);
        // id=1 matched
        assert_eq!(rows[0], vec![Value::BigInt(1), Value::BigInt(1), Value::BigInt(100)]);
        // id=2 unmatched -> nulls for right side
        assert_eq!(rows[1], vec![Value::BigInt(2), Value::Null(crate::core::value::NullType::Null), Value::Null(crate::core::value::NullType::Null)]);
        // id=3 unmatched -> nulls for right side
        assert_eq!(rows[2], vec![Value::BigInt(3), Value::Null(crate::core::value::NullType::Null), Value::Null(crate::core::value::NullType::Null)]);
    }

    #[test]
    fn test_hash_shuffle_inner_join_empty_left_partition() {
        fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
            let mut all_rows = Vec::new();
            while let Ok(Some(chunk)) = join.advance() {
                all_rows.extend(chunk.rows);
            }
            all_rows
        }
        let p0 = scan_executor(vec![], 0, vec!["id".to_string()]);
        let p1 = scan_executor(
            vec![vec![Value::BigInt(1)]],
            1,
            vec!["id".to_string()],
        );
        let r0 = scan_executor(
            vec![vec![Value::BigInt(1), Value::BigInt(100)]],
            0,
            vec!["id".to_string(), "score".to_string()],
        );
        let r1 = scan_executor(
            vec![],
            1,
            vec!["id".to_string(), "score".to_string()],
        );

        let mut join = make_hash_shuffle_join(
            vec![p0, p1],
            vec![r0, r1],
            HashJoinKind::Inner,
            "id", "id",
            vec!["id".to_string()],
            vec!["id".to_string(), "score".to_string()],
            4,
        );

        join.open().expect("empty left join open");
        let rows = collect_all(&mut join);
        join.close().expect("empty left join close");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec![Value::BigInt(1), Value::BigInt(1), Value::BigInt(100)]);
    }

    #[test]
    fn test_hash_shuffle_join_duplicate_keys() {
        let p0 = scan_executor(
            vec![
                vec![Value::BigInt(1), Value::String("a".to_string())],
                vec![Value::BigInt(1), Value::String("b".to_string())],
            ],
            0,
            vec!["id".to_string(), "val".to_string()],
        );
        let p1 = scan_executor(
            vec![vec![Value::BigInt(2), Value::String("c".to_string())]],
            1,
            vec!["id".to_string(), "val".to_string()],
        );

        let r0 = scan_executor(
            vec![vec![Value::BigInt(1), Value::BigInt(100)]],
            0,
            vec!["id".to_string(), "score".to_string()],
        );
        let r1 = scan_executor(
            vec![],
            1,
            vec!["id".to_string(), "score".to_string()],
        );

        let mut join = make_hash_shuffle_join(
            vec![p0, p1],
            vec![r0, r1],
            HashJoinKind::Inner,
            "id", "id",
            vec!["id".to_string(), "val".to_string()],
            vec!["id".to_string(), "score".to_string()],
            4,
        );

        fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
            let mut all_rows = Vec::new();
            while let Ok(Some(chunk)) = join.advance() {
                all_rows.extend(chunk.rows);
            }
            all_rows
        }
        join.open().expect("duplicate keys join open");
        let rows = collect_all(&mut join);
        join.close().expect("duplicate keys join close");

        // Two left rows with id=1, one right row with id=1 → 2 matched rows
        // One left row with id=2, no right row with id=2 → no match
        assert_eq!(rows.len(), 2);
        // Both matched rows should have id=1
        for row in &rows {
            match row.first() {
                Some(Value::BigInt(1)) => {}
                _ => panic!("Expected all rows to have id=1"),
            }
        }
    }

    #[test]
    fn test_two_level_topn_preserves_order_with_larger_limit() {
        let p0 = scan_executor(
            vec![vec![Value::BigInt(10)], vec![Value::BigInt(30)]],
            0,
            vec!["val".to_string(),
        ]);
        let p1 = scan_executor(
            vec![vec![Value::BigInt(20)], vec![Value::BigInt(40)]],
            1,
            vec!["val".to_string()],
        );

        let limit: u32 = 4;
        let sort_expressions = vec![Expression::Variable("val".to_string())];
        let sort_directions = vec![SortDirection::Ascending];

        let local_topns: Vec<StreamingExecutor> = vec![p0, p1]
            .into_iter()
            .map(|tree| {
                StreamingExecutor::Blocking(
                    OperatorBase::new(1),
                    Box::new(tree),
                    BlockingOperator::TopN {
                        n: limit,
                        sort_expressions: sort_expressions.clone(),
                        sort_directions: sort_directions.clone(),
                        memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                        state: None,
                    },
                )
            })
            .collect();

        let mut executor = StreamingExecutor::Gather(
            OperatorBase::new(2).with_global(true),
            local_topns,
            GatherOperator::merge_sort(
                sort_expressions,
                sort_directions,
                Some(limit as usize),
            ),
        );

        executor.open().expect("topn pipeline open");
        let chunk = executor.advance().expect("topn advance");
        executor.close().expect("topn close");

        let chunk = chunk.expect("topn should produce rows");
        let values: Vec<i64> = chunk
            .rows
            .iter()
            .filter_map(|row| match row.first() {
                Some(Value::BigInt(n)) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(values, vec![10, 20, 30, 40], "two-level TopN(4) should produce all sorted values");
    }
}
