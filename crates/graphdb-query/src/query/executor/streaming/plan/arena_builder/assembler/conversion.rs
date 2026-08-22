//! Recursive assembly of operators and fragment DAG edges.

use super::super::super::super::operators::spec::{
    ApplySpec, BlockingSpec, DdlSpec, JoinSpec, SetSpec, SourceSpec, TxnSpec,
};
use super::super::super::super::subquery::SubqueryRunnerSpec;
use super::super::super::properties::{PhysicalProperties, SPILL_DEFAULT_THRESHOLD};
use super::super::super::types::{
    FragmentId, FragmentKind, FragmentSpec, InputContract, LogicalNodeId, OperatorKindSpec,
    PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec, StateOwnership,
};
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::plan::PhysicalPlanBuildContext;
use crate::query::executor::streaming::plan::PhysicalPlanBuilder;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use std::sync::Arc;

use super::super::metadata::{
    estimate_source_cardinality, source_explain_name, source_output_layout,
};
use super::super::specs::*;
use super::fragment_ops::FragmentCtx;
use super::{ArenaFragmentAllocator, ArenaPlanAssembler};

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
            PlanNodeEnum::InnerJoin(join_node) => {
                let (left_fid, _) = Self::convert_node(
                    join_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    join_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = match exec_ctx.join_algorithms.get(&node.id()) {
                    Some(crate::query::optimizer::JoinAlgorithm::NestedLoopJoin { .. }) => {
                        build_inner_join_nested_loop_spec(join_node)?
                    }
                    Some(crate::query::optimizer::JoinAlgorithm::HashJoin { .. }) => {
                        build_inner_join_hash_spec(join_node)?
                    }
                    _ => build_inner_join_spec(join_node)?,
                };
                let (fid, op_id) = Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    spec,
                )?;
                operators[op_id.0].has_folded_expressions = join_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::LeftJoin(join_node) => {
                let (left_fid, _) = Self::convert_node(
                    join_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    join_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = match exec_ctx.join_algorithms.get(&node.id()) {
                    Some(crate::query::optimizer::JoinAlgorithm::NestedLoopJoin { .. }) => {
                        build_left_join_nested_loop_spec(join_node)?
                    }
                    Some(crate::query::optimizer::JoinAlgorithm::HashJoin { .. }) => {
                        build_left_join_hash_spec(join_node)?
                    }
                    _ => build_left_join_spec(join_node)?,
                };
                let (fid, op_id) = Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    spec,
                )?;
                operators[op_id.0].has_folded_expressions = join_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::CrossJoin(join_node) => {
                let (left_fid, _) = Self::convert_node(
                    join_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    join_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    JoinSpec::CrossJoin,
                )
            }
            PlanNodeEnum::RightJoin(join_node) => {
                let (left_fid, _) = Self::convert_node(
                    join_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    join_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_right_join_spec(join_node)?;
                let (fid, op_id) = Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    spec,
                )?;
                operators[op_id.0].has_folded_expressions = join_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::FullOuterJoin(join_node) => {
                let (left_fid, _) = Self::convert_node(
                    join_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    join_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_full_outer_join_spec(join_node)?;
                let (fid, op_id) = Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    spec,
                )?;
                operators[op_id.0].has_folded_expressions = join_node.has_folded_expressions();
                Ok((fid, op_id))
            }
            PlanNodeEnum::SemiJoin(join_node) => {
                let (left_fid, _) = Self::convert_node(
                    join_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    join_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_semi_join_spec(join_node)?;
                let (fid, op_id) = Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    spec,
                )?;
                operators[op_id.0].has_folded_expressions = join_node.has_folded_expressions();
                Ok((fid, op_id))
            }

            // ── Set/Apply nodes ─────────────────────────────────────────────────
            PlanNodeEnum::Union(union_node) => {
                let (left_fid, _) = Self::convert_node(
                    union_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    union_node.union_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let set_spec = if union_node.distinct() {
                    SetSpec::Union
                } else {
                    SetSpec::UnionAll
                };
                Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    set_spec,
                )
            }
            PlanNodeEnum::Minus(minus_node) => {
                let (left_fid, _) = Self::convert_node(
                    minus_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    minus_node.minus_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    SetSpec::Minus,
                )
            }
            PlanNodeEnum::Intersect(intersect_node) => {
                let (left_fid, _) = Self::convert_node(
                    intersect_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    intersect_node.intersect_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    SetSpec::Intersect,
                )
            }
            PlanNodeEnum::PatternApply(pa_node) => {
                let (left_fid, _) = Self::convert_node(
                    pa_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    pa_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_pattern_apply_spec(pa_node)?;
                Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::CorrelatedApply(ca_node) => {
                let (left_fid, _) = Self::convert_node(
                    ca_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                // Build the self-contained right subtree as a nested physical
                // plan (Argument -> ... -> Filter over the correlation frame).
                // The sub-plan stays out of the outer fragment graph: only the
                // left input participates in the arena DAG, and the right
                // subtree is re-executed per row at runtime.
                let mut sub_ctx = PhysicalPlanBuildContext::from_execution_context(exec_ctx);
                sub_ctx.partition_spec = None;
                let sub_plan = Arc::new(PhysicalPlanBuilder::build(
                    ca_node.right_input(),
                    &mut sub_ctx,
                    exec_ctx,
                )?);
                let spec = ApplySpec::CorrelatedApply {
                    sub_plan,
                    anti: ca_node.is_anti_predicate(),
                };
                Self::push_apply_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    left_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::RollUpApply(rua_node) => {
                let (left_fid, _) = Self::convert_node(
                    rua_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    rua_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_rollup_apply_spec(rua_node)?;
                Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::Apply(apply_node) => {
                let (left_fid, _) = Self::convert_node(
                    apply_node.left_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let (right_fid, _) = Self::convert_node(
                    apply_node.right_input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_apply_spec(apply_node)?;
                Self::push_binary_op(
                    &mut FragmentCtx {
                        operators,
                        fragments,
                        op_alloc,
                    },
                    frag_alloc,
                    left_fid,
                    right_fid,
                    node.id(),
                    spec,
                )
            }

            // ── Write/sink nodes ────────────────────────────────────────────────
            PlanNodeEnum::InsertVertices(iv_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_insert_vertices_spec(iv_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::InsertEdges(ie_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_insert_edges_spec(ie_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::DeleteVertices(dv_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_delete_vertices_spec(dv_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::DeleteEdges(de_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_delete_edges_spec(de_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::DeleteTags(dt_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_delete_tags_spec(dt_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::PipeDeleteVertices(pdv_node) => {
                let (child_fid, _) = Self::convert_node(
                    pdv_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_pipe_delete_vertices_spec(pdv_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::PipeDeleteEdges(pde_node) => {
                let (child_fid, _) = Self::convert_node(
                    pde_node.input(),
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_pipe_delete_edges_spec(pde_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::Update(u_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_update_spec(u_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::UpdateVertices(uv_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_update_vertices_spec(uv_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::UpdateEdges(ue_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_update_edges_spec(ue_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::CopyFrom(copy_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_copy_from_spec(copy_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::CopyTo(copy_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    build_standalone_write_source(node)?,
                );
                let spec = build_copy_to_spec(copy_node, exec_ctx)?;
                Self::push_sink_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }

            // ── DDL nodes ───────────────────────────────────────────────────────
            PlanNodeEnum::SpaceManage(sm_node) => {
                let spec = build_space_manage_spec(sm_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::TagManage(tm_node) => {
                let spec = build_tag_manage_spec(tm_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::EdgeManage(em_node) => {
                let spec = build_edge_manage_spec(em_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::IndexManage(im_node) => {
                let spec = build_index_manage_spec(im_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::DeleteIndex(di_node) => {
                let spec = build_delete_index_spec(di_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::UserManage(um_node) => {
                let spec = build_user_manage_spec(um_node, exec_ctx)?;
                Self::push_ddl_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::ShowStats(_) => Self::push_ddl_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                DdlSpec::ShowStats {
                    space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                },
            ),
            PlanNodeEnum::ShowConfigs(_) => Self::push_ddl_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                DdlSpec::ShowConfigs {
                    space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                },
            ),
            PlanNodeEnum::ShowQueries(_) => Self::push_ddl_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                DdlSpec::ShowQueries {
                    space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                },
            ),
            PlanNodeEnum::ShowSessions(_) => Self::push_ddl_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                DdlSpec::ShowSessions {
                    space_name: exec_ctx.space_name.clone().unwrap_or_default(),
                },
            ),

            // ── Fulltext nodes ──────────────────────────────────────────────────
            PlanNodeEnum::FulltextManage(fm_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_fulltext_manage_spec(fm_node, exec_ctx)?;
                Self::push_fulltext_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::FulltextSearch(fs_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_fulltext_search_spec(fs_node, exec_ctx)?;
                Self::push_fulltext_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::FulltextLookup(fl_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_fulltext_lookup_spec(fl_node, exec_ctx)?;
                Self::push_fulltext_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            PlanNodeEnum::MatchFulltext(mf_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_match_fulltext_spec(mf_node, exec_ctx)?;
                Self::push_fulltext_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }

            // ── Vector nodes ────────────────────────────────────────────────────
            PlanNodeEnum::VectorManage(vm_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_vector_manage_spec(vm_node, exec_ctx)?;
                Self::push_vector_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            #[cfg(feature = "vector")]
            PlanNodeEnum::VectorSearch(vs_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_vector_search_spec(vs_node, exec_ctx)?;
                Self::push_vector_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            #[cfg(feature = "vector")]
            PlanNodeEnum::VectorLookup(vl_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_vector_lookup_spec(vl_node, exec_ctx)?;
                Self::push_vector_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }
            #[cfg(feature = "vector")]
            PlanNodeEnum::VectorMatch(vm_node) => {
                let (child_fid, _) = Self::push_source_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    node.id(),
                    SourceSpec::Start,
                );
                let spec = build_vector_match_spec(vm_node, exec_ctx)?;
                Self::push_vector_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    node.id(),
                    spec,
                )
            }

            // ── Transaction nodes ───────────────────────────────────────────────
            PlanNodeEnum::BeginTransaction(_) => Self::push_txn_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                TxnSpec::BeginTransaction,
            ),
            PlanNodeEnum::Commit(_) => Self::push_txn_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                TxnSpec::Commit,
            ),
            PlanNodeEnum::Rollback(rollback_node) => {
                let spec = match rollback_node.savepoint() {
                    Some(name) => TxnSpec::RollbackToSavepoint {
                        name: name.to_string(),
                    },
                    None => TxnSpec::Rollback,
                };
                Self::push_txn_op(operators, fragments, op_alloc, frag_alloc, node.id(), spec)
            }
            PlanNodeEnum::Savepoint(savepoint_node) => Self::push_txn_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                TxnSpec::Savepoint {
                    name: savepoint_node.savepoint().to_string(),
                },
            ),
            PlanNodeEnum::ReleaseSavepoint(release_node) => Self::push_txn_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                node.id(),
                TxnSpec::ReleaseSavepoint {
                    name: release_node.savepoint().to_string(),
                },
            ),

            // ── Unsupported ─────────────────────────────────────────────────────
            PlanNodeEnum::Loop(_) => Err(PlanBuildError::unsupported(
                node.name(),
                node.id(),
                "Loop plan nodes are not supported",
            )),
            PlanNodeEnum::PassThrough(_) => Err(PlanBuildError::unsupported(
                node.name(),
                node.id(),
                "PassThrough plan nodes are not supported",
            )),
            PlanNodeEnum::Select(_) => Err(PlanBuildError::unsupported(
                node.name(),
                node.id(),
                "Select plan nodes are not supported",
            )),
            PlanNodeEnum::AppendVertices(append_node) => {
                let (child_fid, _) = Self::convert_node(
                    append_node.inputs().first().ok_or_else(|| {
                        PlanBuildError::missing_value(
                            "AppendVertices",
                            node.id(),
                            "input",
                            "AppendVertices requires an input",
                        )
                    })?,
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    exec_ctx,
                )?;
                let spec = build_append_vertices_spec(append_node, exec_ctx)?;
                Self::push_unary_op(operators, fragments, op_alloc, child_fid, node.id(), spec)
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
    planned: &[crate::query::planning::statements::clauses::exists_planner::PlannedSubquery],
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
                crate::query::executor::streaming::subquery::GroupJoinSpec {
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
