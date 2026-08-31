//! Recursive assembly of operators and fragment DAG edges.

use super::super::super::super::operators::spec::BlockingSpec;
use super::super::super::super::subquery::SubqueryRunnerSpec;
use super::super::super::properties::{PhysicalProperties, SPILL_DEFAULT_THRESHOLD};
use super::super::super::types::{
    FragmentId, FragmentKind, FragmentSpec, InputContract, LogicalNodeId, OperatorKindSpec,
    PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec, StateOwnership,
};
use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::plan::PhysicalPlanBuildContext;
use crate::executor::streaming::plan::PhysicalPlanBuilder;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use std::sync::Arc;

use super::super::metadata::{
    estimate_source_cardinality, source_explain_name, source_output_layout,
};
use super::super::specs::*;
use super::fragment_ops::FragmentCtx;
use super::{ArenaFragmentAllocator, ArenaPlanAssembler};

mod binary;
mod write;

impl ArenaPlanAssembler {
    pub(crate) fn convert_node(
        node: &PlanNodeEnum,
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        exec_ctx: &ExecutionContext,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        match node {
            // ── Source nodes ────────────────────────────────────────────────────
            PlanNodeEnum::Start(_)
            | PlanNodeEnum::Argument(_)
            | PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::GetVertices(_)
            | PlanNodeEnum::GetEdges(_)
            | PlanNodeEnum::GetNeighbors(_)
            | PlanNodeEnum::IndexScan(_) => {
                let spec = build_source_spec(node, exec_ctx)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();
                let output_layout = source_output_layout(&spec);

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(node.id())),
                    spec: OperatorKindSpec::Source(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout,
                    properties: PhysicalProperties::single_streaming(),
                    state_ownership: StateOwnership::TreeLocal,
                    estimated_cardinality: estimate_source_cardinality(&spec),
                    choice_reason: None,
                    has_folded_expressions: false,
                    explain_name: source_explain_name(&spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Source,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: Vec::new(),
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            // ── Unary nodes ─────────────────────────────────────────────────────
            PlanNodeEnum::Filter(filter_node) => {
                let (child_fid, _) = Self::convert_node(
                    filter_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let subquery_runners =
                    build_subquery_runner_specs(filter_node.subqueries(), exec_ctx)?;
                let spec = build_filter_spec(filter_node, subquery_runners)?;
                let (fid, op_id) = Self::push_unary_op(
                    operators,
                    fragments,
                    op_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )?;
                operators[op_id.0].has_folded_expressions = filter_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::Project(project_node) => {
                let (child_fid, _) = Self::convert_node(
                    project_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let subquery_runners =
                    build_subquery_runner_specs(project_node.subqueries(), exec_ctx)?;
                let spec = build_project_spec(project_node, subquery_runners)?;
                let (fid, op_id) = Self::push_unary_op(
                    operators,
                    fragments,
                    op_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )?;
                operators[op_id.0].has_folded_expressions = project_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::Limit(limit_node) => {
                let (child_fid, _) = Self::convert_node(
                    limit_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_limit_spec(limit_node)?;
                Self::push_unary_op(operators, fragments, op_alloc, child_fid, node.id(), spec)
            }
            PlanNodeEnum::Sample(sample_node) => {
                let (child_fid, _) = Self::convert_node(
                    sample_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_sample_spec(sample_node)?;
                Self::push_unary_op(operators, fragments, op_alloc, child_fid, node.id(), spec)
            }
            PlanNodeEnum::Remove(remove_node) => {
                let (child_fid, _) = Self::convert_node(
                    remove_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_remove_spec(remove_node)?;
                Self::push_unary_op(operators, fragments, op_alloc, child_fid, node.id(), spec)
            }
            PlanNodeEnum::Assign(assign_node) => {
                let (child_fid, _) = Self::convert_node(
                    assign_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let subquery_runners =
                    build_subquery_runner_specs(assign_node.subqueries(), exec_ctx)?;
                let spec = build_assign_spec(assign_node, subquery_runners)?;
                let (fid, op_id) = Self::push_unary_op(
                    operators,
                    fragments,
                    op_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )?;
                operators[op_id.0].has_folded_expressions = assign_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::Unwind(unwind_node) => {
                let (child_fid, _) = Self::convert_node(
                    unwind_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_unwind_spec(unwind_node)?;
                Self::push_unary_op(operators, fragments, op_alloc, child_fid, node.id(), spec)
            }

            // ── Blocking nodes ──────────────────────────────────────────────────
            PlanNodeEnum::Sort(sort_node) => {
                let (child_fid, _) = Self::convert_node(
                    sort_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_sort_spec(sort_node)?;
                let (fid, op_id) = Self::push_blocking_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    child_fid,
                    node.id(),
                    spec,
                    PhysicalProperties::single_blocking_spillable(SPILL_DEFAULT_THRESHOLD),
                )?;
                operators[op_id.0].has_folded_expressions = sort_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::Aggregate(agg_node) => {
                let (child_fid, _) = Self::convert_node(
                    agg_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_aggregate_spec(agg_node)?;
                let (fid, op_id) = Self::push_blocking_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    child_fid,
                    node.id(),
                    spec,
                    PhysicalProperties::single_blocking_with_budget(),
                )?;
                operators[op_id.0].has_folded_expressions = agg_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::Dedup(dedup_node) => {
                let (child_fid, _) = Self::convert_node(
                    dedup_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                Self::push_blocking_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    child_fid,
                    node.id(),
                    BlockingSpec::Distinct,
                    PhysicalProperties::single_blocking_with_budget(),
                )
            }
            PlanNodeEnum::TopN(topn_node) => {
                let (child_fid, _) = Self::convert_node(
                    topn_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_topn_spec(topn_node)?;
                Self::push_blocking_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    child_fid,
                    node.id(),
                    spec,
                    PhysicalProperties::single_blocking_with_budget(),
                )
            }
            PlanNodeEnum::Window(window_node) => {
                let (child_fid, _) = Self::convert_node(
                    window_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_window_spec(window_node)?;
                let (fid, op_id) = Self::push_blocking_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    child_fid,
                    node.id(),
                    spec,
                    PhysicalProperties::single_blocking_with_budget(),
                )?;
                operators[op_id.0].has_folded_expressions = window_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::DataCollect(collect_node) => {
                let (child_fid, _) = Self::convert_node(
                    collect_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                Self::push_blocking_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    child_fid,
                    node.id(),
                    BlockingSpec::DataCollect,
                    PhysicalProperties::single_blocking_with_budget(),
                )
            }
            PlanNodeEnum::Materialize(mat_node) => {
                let (child_fid, _) = Self::convert_node(
                    mat_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                Self::push_blocking_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    child_fid,
                    node.id(),
                    BlockingSpec::Materialize,
                    PhysicalProperties::single_blocking_with_budget(),
                )
            }

            // ── Graph nodes ─────────────────────────────────────────────────────
            PlanNodeEnum::Expand(expand_node) => {
                let (child_fid, _) = Self::convert_node(
                    expand_node.inputs().first().ok_or_else(|| {
                        PlanBuildError::missing_value(
                            "Expand",
                            node.id(),
                            "input",
                            "Expand requires an input",
                        )
                    })?,
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_expand_spec(expand_node, exec_ctx)?;
                Self::push_graph_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::ExpandAll(expand_node) => {
                let (child_fid, _) = Self::convert_node(
                    expand_node.inputs().first().ok_or_else(|| {
                        PlanBuildError::missing_value(
                            "ExpandAll",
                            node.id(),
                            "input",
                            "ExpandAll requires an input",
                        )
                    })?,
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_expand_all_spec(expand_node, exec_ctx)?;
                Self::push_graph_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::Traverse(traverse_node) => {
                let (child_fid, _) = Self::convert_node(
                    traverse_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_traverse_spec(traverse_node, exec_ctx)?;
                Self::push_graph_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::BiExpand(bi_expand_node) => {
                let (child_fid, _) = Self::convert_node(
                    bi_expand_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_bi_expand_spec(bi_expand_node, exec_ctx)?;
                Self::push_graph_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::BiTraverse(bi_traverse_node) => {
                let (child_fid, _) = Self::convert_node(
                    bi_traverse_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_bi_traverse_spec(bi_traverse_node, exec_ctx)?;
                Self::push_graph_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }

            // ── Recursive fragment nodes ────────────────────────────────────────
            PlanNodeEnum::ShortestPath(sp_node) => {
                let (child_fid, _) = Self::convert_node(
                    sp_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_shortest_path_spec(sp_node, exec_ctx)?;
                Self::push_recursive_fragment_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::MultiShortestPath(msp_node) => {
                let (child_fid, _) = Self::convert_node(
                    msp_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_multi_shortest_path_spec(msp_node, exec_ctx)?;
                Self::push_recursive_fragment_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::BFSShortest(bfs_node) => {
                let (child_fid, _) = Self::convert_node(
                    bfs_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_bfs_shortest_spec(bfs_node, exec_ctx)?;
                Self::push_recursive_fragment_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::AllPaths(ap_node) => {
                let (child_fid, _) = Self::convert_node(
                    ap_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_all_paths_spec(ap_node, exec_ctx)?;
                Self::push_recursive_fragment_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }

            // ── Binary nodes (join/set/apply) ───────────────────────────────────
            PlanNodeEnum::InnerJoin(_)
            | PlanNodeEnum::LeftJoin(_)
            | PlanNodeEnum::CrossJoin(_)
            | PlanNodeEnum::RightJoin(_)
            | PlanNodeEnum::FullOuterJoin(_)
            | PlanNodeEnum::SemiJoin(_)
            | PlanNodeEnum::Union(_)
            | PlanNodeEnum::Minus(_)
            | PlanNodeEnum::Intersect(_)
            | PlanNodeEnum::PatternApply(_)
            | PlanNodeEnum::CorrelatedApply(_)
            | PlanNodeEnum::RollUpApply(_)
            | PlanNodeEnum::Apply(_) => {
                if matches!(
                    node,
                    PlanNodeEnum::InnerJoin(_)
                        | PlanNodeEnum::LeftJoin(_)
                        | PlanNodeEnum::CrossJoin(_)
                        | PlanNodeEnum::RightJoin(_)
                        | PlanNodeEnum::FullOuterJoin(_)
                        | PlanNodeEnum::SemiJoin(_)
                ) {
                    Self::convert_join_node(
                        node, operators, fragments, op_alloc, frag_alloc, exec_ctx,
                    )
                } else {
                    Self::convert_set_apply_node(
                        node, operators, fragments, op_alloc, frag_alloc, exec_ctx,
                    )
                }
            }

            // ── Write/sink nodes ────────────────────────────────────────────────
            PlanNodeEnum::InsertVertices(_)
            | PlanNodeEnum::InsertEdges(_)
            | PlanNodeEnum::DeleteVertices(_)
            | PlanNodeEnum::DeleteEdges(_)
            | PlanNodeEnum::DeleteTags(_)
            | PlanNodeEnum::PipeDeleteVertices(_)
            | PlanNodeEnum::PipeDeleteEdges(_)
            | PlanNodeEnum::Update(_)
            | PlanNodeEnum::UpdateVertices(_)
            | PlanNodeEnum::UpdateEdges(_)
            | PlanNodeEnum::CopyFrom(_)
            | PlanNodeEnum::CopyTo(_)
            | PlanNodeEnum::SpaceManage(_)
            | PlanNodeEnum::TagManage(_)
            | PlanNodeEnum::EdgeManage(_)
            | PlanNodeEnum::IndexManage(_)
            | PlanNodeEnum::DeleteIndex(_)
            | PlanNodeEnum::UserManage(_)
            | PlanNodeEnum::ShowStats(_)
            | PlanNodeEnum::ShowConfigs(_)
            | PlanNodeEnum::ShowQueries(_)
            | PlanNodeEnum::ShowSessions(_)
            | PlanNodeEnum::FulltextManage(_)
            | PlanNodeEnum::FulltextSearch(_)
            | PlanNodeEnum::FulltextLookup(_)
            | PlanNodeEnum::MatchFulltext(_)
            | PlanNodeEnum::VectorManage(_)
            | PlanNodeEnum::BeginTransaction(_)
            | PlanNodeEnum::Commit(_)
            | PlanNodeEnum::Rollback(_)
            | PlanNodeEnum::Savepoint(_)
            | PlanNodeEnum::ReleaseSavepoint(_)
            | PlanNodeEnum::Loop(_)
            | PlanNodeEnum::PassThrough(_)
            | PlanNodeEnum::Select(_)
            | PlanNodeEnum::AppendVertices(_) => {
                Self::convert_write_node(node, operators, fragments, op_alloc, frag_alloc, exec_ctx)
            }
            #[cfg(feature = "vector")]
            PlanNodeEnum::VectorSearch(_)
            | PlanNodeEnum::VectorLookup(_)
            | PlanNodeEnum::VectorMatch(_) => {
                Self::convert_write_node(node, operators, fragments, op_alloc, frag_alloc, exec_ctx)
            }
        }
    }
}

/// Compile expression-level subqueries into immutable runner specs.
///
/// Each `PlannedSubquery`'s plan root is built as a self-contained physical
/// plan (same `sub_ctx` convention as the CorrelatedApply right subtree); the
/// materializer later instantiates per-operator runners from these specs.
pub(crate) fn build_subquery_runner_specs(
    planned: &[crate::planning::statements::clauses::exists_planner::PlannedSubquery],
    exec_ctx: &ExecutionContext,
) -> Result<Vec<SubqueryRunnerSpec>, PlanBuildError> {
    let mut specs = Vec::with_capacity(planned.len());
    for subquery in planned {
        let root = subquery.plan.root().clone().ok_or_else(|| {
            PlanBuildError::missing_value(
                "Subquery",
                -1,
                "root",
                "expression-level subquery plan has no root node",
            )
        })?;
        let mut sub_ctx = PhysicalPlanBuildContext::from_execution_context(exec_ctx);
        sub_ctx.partition_spec = None;
        let sub_plan = Arc::new(PhysicalPlanBuilder::build(&root, &mut sub_ctx, exec_ctx)?);
        specs.push(SubqueryRunnerSpec {
            id: subquery.id,
            plan: sub_plan,
            correlated: subquery.correlated,
            group_join: subquery.group_join.clone().map(|gj| {
                crate::executor::streaming::subquery::GroupJoinSpec {
                    hash_keys: gj.hash_keys,
                    key_columns: gj.key_columns,
                    function: gj.function,
                    distinct: gj.distinct,
                }
            }),
        });
    }
    Ok(specs)
}
