use std::sync::Arc;

use super::super::super::super::super::operators::spec::{ApplySpec, JoinSpec, SetSpec};
use super::super::super::super::types::{
    FragmentId, FragmentSpec, PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec,
};
use super::super::super::specs::*;
use super::super::fragment_ops::FragmentCtx;
use super::super::{ArenaFragmentAllocator, ArenaPlanAssembler};
use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::plan::PhysicalPlanBuildContext;
use crate::executor::streaming::plan::PhysicalPlanBuilder;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

impl ArenaPlanAssembler {
    pub(crate) fn convert_join_node(
        node: &PlanNodeEnum,
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        exec_ctx: &ExecutionContext,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        match node {
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
                    Some(crate::optimizer::JoinAlgorithm::NestedLoopJoin { .. }) => {
                        build_inner_join_nested_loop_spec(join_node)?
                    }
                    Some(crate::optimizer::JoinAlgorithm::HashJoin { .. }) => {
                        build_inner_join_hash_spec(join_node)?
                    }
                    // No cost-based decision: valid equi keys take the hash
                    // join form (linear instead of quadratic); otherwise the
                    // condition (nested-loop) default applies. The
                    // partitioned join paths keep using
                    // `build_inner_join_spec` explicitly because each
                    // partition-local join must carry the equality condition.
                    _ => build_join_with_keys(
                        join_node.hash_keys(),
                        join_node.probe_keys(),
                        JoinSpec::InnerJoin {
                            join_condition: None,
                        },
                    )?,
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
                    Some(crate::optimizer::JoinAlgorithm::NestedLoopJoin { .. }) => {
                        build_left_join_nested_loop_spec(join_node)?
                    }
                    Some(crate::optimizer::JoinAlgorithm::HashJoin { .. }) => {
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
            _ => unreachable!("convert_join_node called with non-join node"),
        }
    }

    pub(crate) fn convert_set_apply_node(
        node: &PlanNodeEnum,
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        exec_ctx: &ExecutionContext,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        match node {
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
            _ => unreachable!("convert_set_apply_node called with non-set/apply node"),
        }
    }
}
