//! Builds physical streaming executor trees from partitioned physical plans.
//!
//! The recursive builder in this module converts a [`PartitionedPhysicalNode`]
//! tree into [`BuildOutput`] — either a single global executor or a set of
//! per-partition local trees — which the caller gathers into a final root.

use std::sync::Arc;

use super::super::builder::StreamingExecutorBuilder;
use super::super::executor::StreamingExecutor;
use super::super::operator_plan_builder::relational;
use super::super::operators::base::OperatorBase;
use super::super::operators::blocking::BlockingOperator;
use super::super::operators::gather_operator::GatherOperator;
use super::super::operators::shuffle_join_operator::{HashJoinKind, HashShuffleJoinOperator};
use super::super::partition::PartitionView;
use super::super::partition_builder;
use super::super::runtime::ExecutionRuntime;
use super::types::SyntheticNodeIdAllocator;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::query::executor::base::{ExecutionContext, MemoryTracker};
use crate::query::planning::plan::{PartitionedPhysicalNode, PlanNodeEnum};

fn allocate_gather_node_id(alloc: &mut SyntheticNodeIdAllocator) -> i64 {
    alloc.allocate()
}

fn require_partition_local(
    local_trees: &[StreamingExecutor],
    context: &str,
    plan_name: &str,
) -> Result<(), QueryError> {
    for executor in local_trees {
        if !executor.is_partition_local() {
            return Err(QueryError::execution(format!(
                "{} local subtree '{}' is not partition-local",
                context, plan_name
            )));
        }
    }
    Ok(())
}

/// Intermediate build result for the builder's recursive construction.
/// - `Global`: a single executor tree (result of a global or exchange operator).
/// - `Local`: a set of per-partition trees that have not yet been gathered.
pub(crate) enum BuildOutput {
    Global(Box<StreamingExecutor>),
    Local(Vec<StreamingExecutor>),
}

/// Build a partitioned physical node into a `BuildOutput`.
///
/// Recursively descends the [`PartitionedPhysicalNode`] tree.  `Local` nodes
/// produce per-partition trees; `GlobalUnary`, `GlobalBinary`,
/// `AggregateSplit`, `DistinctSplit`, `TopNSplit` and `HashJoinExchange`
/// produce a single global tree.
///
/// When `runtime` is `Some`, all materialized executors share that runtime.
/// When `None`, each materialization creates its own (legacy path).
pub(crate) fn build_partitioned_physical_node(
    node: &PartitionedPhysicalNode,
    context: &ExecutionContext,
    partition_view: &PartitionView,
    synthetic_id_alloc: &mut SyntheticNodeIdAllocator,
    runtime: Option<Arc<ExecutionRuntime>>,
) -> Result<BuildOutput, QueryError> {
    match node {
        PartitionedPhysicalNode::Local { logical_plan } => {
            let mut local_trees = partition_builder::build_partitioned(
                logical_plan,
                context,
                partition_view,
                runtime,
            )?;
            require_partition_local(&local_trees, "Physical", logical_plan.name())?;
            for (partition_id, tree) in local_trees.iter_mut().enumerate() {
                tree.set_partition_id(partition_id);
            }
            Ok(BuildOutput::Local(local_trees))
        }
        PartitionedPhysicalNode::GlobalUnary {
            logical_plan,
            input,
        } => {
            let input = build_partitioned_physical_node(
                input,
                context,
                partition_view,
                synthetic_id_alloc,
                runtime.clone(),
            )?;
            let input = local_to_global(input, synthetic_id_alloc)?;
            let physical =
                StreamingExecutorBuilder::from_plan_node_physical(logical_plan, context)?;
            let mut global = StreamingExecutorBuilder::materialize_physical(
                &physical,
                runtime.clone(),
                &context.memory_budget,
                context.chunk_size,
            );
            global.set_global();
            replace_single_input(&mut global, input)?;
            Ok(BuildOutput::Global(Box::new(global)))
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
            let local_trees = partition_builder::build_partitioned(
                local_plan,
                context,
                partition_view,
                runtime.clone(),
            )?;
            require_partition_local(&local_trees, "AggregateSplit", local_plan.name())?;

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
                            memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                            state: None,
                        },
                    )
                })
                .collect();

            let gather_node_id = allocate_gather_node_id(synthetic_id_alloc);
            let gather = StreamingExecutor::Gather(
                OperatorBase::new(gather_node_id).with_global(true),
                partial_aggregates,
                GatherOperator::concatenate(),
            );

            let start_id = synthetic_id_alloc.allocate();
            let mut final_aggregate = StreamingExecutor::Blocking(
                OperatorBase::new(aggregate.id()).with_global(true),
                Box::new(StreamingExecutor::Source(
                    OperatorBase::new(start_id),
                    super::super::operators::source_operator::SourceOperator::Start,
                )),
                BlockingOperator::FinalAggregate {
                    group_by_expressions,
                    aggregate_functions,
                    output_col_names,
                    memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                    state: None,
                },
            );
            replace_single_input(&mut final_aggregate, gather)?;
            Ok(BuildOutput::Global(Box::new(final_aggregate)))
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
            let local_trees = partition_builder::build_partitioned(
                input_node,
                context,
                partition_view,
                runtime.clone(),
            )?;
            require_partition_local(&local_trees, "DistinctSplit", input_node.name())?;

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

            let gather_node_id = allocate_gather_node_id(synthetic_id_alloc);
            let gather = StreamingExecutor::Gather(
                OperatorBase::new(gather_node_id).with_global(true),
                local_distincts,
                GatherOperator::concatenate(),
            );

            let start_id = synthetic_id_alloc.allocate();
            let mut global_distinct = StreamingExecutor::Blocking(
                OperatorBase::new(logical_plan.id()).with_global(true),
                Box::new(StreamingExecutor::Source(
                    OperatorBase::new(start_id),
                    super::super::operators::source_operator::SourceOperator::Start,
                )),
                BlockingOperator::Distinct {
                    memory_tracker,
                    state: None,
                },
            );
            replace_single_input(&mut global_distinct, gather)?;
            Ok(BuildOutput::Global(Box::new(global_distinct)))
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
            let local_trees = partition_builder::build_partitioned(
                input_node,
                context,
                partition_view,
                runtime.clone(),
            )?;
            require_partition_local(&local_trees, "TopNSplit", input_node.name())?;

            let limit = topn_node.limit() as u32;
            let sort_items = topn_node.sort_items();
            let (sort_expressions, sort_directions) =
                relational::sort_items_to_expressions(sort_items)
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
                            memory_tracker: MemoryTracker::new(context.memory_budget.clone()),
                            state: None,
                        },
                    )
                })
                .collect();

            let gather_node_id = allocate_gather_node_id(synthetic_id_alloc);
            Ok(BuildOutput::Global(Box::new(StreamingExecutor::Gather(
                OperatorBase::new(gather_node_id).with_global(true),
                local_topns,
                GatherOperator::merge_sort(sort_expressions, sort_directions, Some(limit as usize)),
            ))))
        }
        PartitionedPhysicalNode::HashJoinExchange {
            logical_plan,
            left,
            right,
            bucket_count,
        } => {
            let (left_input, right_input) = match (left.as_ref(), right.as_ref()) {
                (PartitionedPhysicalNode::Local { .. }, PartitionedPhysicalNode::Local { .. }) => {
                    let left_output = build_partitioned_physical_node(
                        left,
                        context,
                        partition_view,
                        synthetic_id_alloc,
                        runtime.clone(),
                    )?;
                    let right_output = build_partitioned_physical_node(
                        right,
                        context,
                        partition_view,
                        synthetic_id_alloc,
                        runtime.clone(),
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

            let to_expr = super::super::operator_plan_builder::contextual_to_expression;

            let (
                left_key_exprs,
                right_key_exprs,
                join_condition,
                join_kind,
                left_schema,
                right_schema,
            ) = match logical_plan {
                PlanNodeEnum::HashInnerJoin(join_node) => {
                    let hash_keys: Vec<Expression> = join_node
                        .hash_keys()
                        .iter()
                        .map(to_expr)
                        .collect::<Result<_, _>>()?;
                    let probe_keys: Vec<Expression> = join_node
                        .probe_keys()
                        .iter()
                        .map(to_expr)
                        .collect::<Result<_, _>>()?;
                    let condition = super::super::operator_plan_builder::join_condition_from_keys(
                        join_node.hash_keys(),
                        join_node.probe_keys(),
                        join_node.right_input().col_names(),
                    )?;
                    let left_schema = join_node.left_input().col_names().to_vec();
                    let right_schema = join_node.right_input().col_names().to_vec();
                    (
                        probe_keys,
                        hash_keys,
                        condition,
                        HashJoinKind::Inner,
                        left_schema,
                        right_schema,
                    )
                }
                PlanNodeEnum::HashLeftJoin(join_node) => {
                    let hash_keys: Vec<Expression> = join_node
                        .hash_keys()
                        .iter()
                        .map(to_expr)
                        .collect::<Result<_, _>>()?;
                    let probe_keys: Vec<Expression> = join_node
                        .probe_keys()
                        .iter()
                        .map(to_expr)
                        .collect::<Result<_, _>>()?;
                    let condition = super::super::operator_plan_builder::join_condition_from_keys(
                        join_node.hash_keys(),
                        join_node.probe_keys(),
                        join_node.right_input().col_names(),
                    )?;
                    let left_schema = join_node.left_input().col_names().to_vec();
                    let right_schema = join_node.right_input().col_names().to_vec();
                    (
                        probe_keys,
                        hash_keys,
                        condition,
                        HashJoinKind::Left,
                        left_schema,
                        right_schema,
                    )
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
            Ok(BuildOutput::Global(Box::new(join_executor)))
        }
        PartitionedPhysicalNode::GlobalBinary {
            logical_plan,
            left,
            right,
        } => {
            let left = build_partitioned_physical_node(
                left,
                context,
                partition_view,
                synthetic_id_alloc,
                runtime.clone(),
            )?;
            let left = local_to_global(left, synthetic_id_alloc)?;
            let right = build_partitioned_physical_node(
                right,
                context,
                partition_view,
                synthetic_id_alloc,
                runtime.clone(),
            )?;
            let right = local_to_global(right, synthetic_id_alloc)?;
            let physical =
                StreamingExecutorBuilder::from_plan_node_physical(logical_plan, context)?;
            let mut global = StreamingExecutorBuilder::materialize_physical(
                &physical,
                runtime,
                &context.memory_budget,
                context.chunk_size,
            );
            global.set_global();
            replace_binary_inputs(&mut global, left, right)?;
            Ok(BuildOutput::Global(Box::new(global)))
        }
    }
}

/// Convert a BuildOutput::Local to a global executor by wrapping with
/// Gather::Concatenate. Identity for BuildOutput::Global.
pub(crate) fn local_to_global(
    output: BuildOutput,
    synthetic_id_alloc: &mut SyntheticNodeIdAllocator,
) -> Result<StreamingExecutor, QueryError> {
    match output {
        BuildOutput::Global(executor) => Ok(*executor),
        BuildOutput::Local(trees) => {
            if trees.is_empty() {
                return Err(QueryError::execution(
                    "Cannot gather empty local trees".to_string(),
                ));
            }
            let gather_node_id = allocate_gather_node_id(synthetic_id_alloc);
            Ok(StreamingExecutor::Gather(
                OperatorBase::new(gather_node_id).with_global(true),
                trees,
                GatherOperator::concatenate(),
            ))
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
        | StreamingExecutor::RecursiveFragment(_, child, _)
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

#[cfg(test)]
#[path = "builder_test.rs"]
mod tests;
