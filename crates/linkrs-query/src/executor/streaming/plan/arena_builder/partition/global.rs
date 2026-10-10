use super::super::super::super::operators::spec::BlockingSpec;
use super::super::super::properties::{PhysicalProperties, SPILL_DEFAULT_THRESHOLD};
use super::super::super::types::{
    FragmentId, FragmentSpec, PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec,
};
use super::super::assembler::{ArenaFragmentAllocator, ArenaPlanAssembler};
use super::super::specs::{
    build_aggregate_spec, build_filter_spec, build_limit_spec, build_project_spec, build_sort_spec,
    build_topn_spec, build_window_spec,
};
use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub(super) fn push_global_op(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    child_fid: FragmentId,
    op: &PlanNodeEnum,
    exec_ctx: &ExecutionContext,
) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
    match op {
        PlanNodeEnum::Filter(filter) => {
            let subquery_runners = super::super::assembler::build_subquery_runner_specs(
                filter.subqueries(),
                exec_ctx,
            )?;
            let spec = build_filter_spec(filter, subquery_runners)?;
            let (fid, op_id) = ArenaPlanAssembler::push_global_unary_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                child_fid,
                op.id(),
                spec,
            )?;
            operators[op_id.0].has_folded_expressions = filter.has_folded_expressions();
            Ok((fid, op_id))
        }
        PlanNodeEnum::Project(project) => {
            let subquery_runners = super::super::assembler::build_subquery_runner_specs(
                project.subqueries(),
                exec_ctx,
            )?;
            let spec = build_project_spec(project, subquery_runners)?;
            let (fid, op_id) = ArenaPlanAssembler::push_global_unary_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                child_fid,
                op.id(),
                spec,
            )?;
            operators[op_id.0].has_folded_expressions = project.has_folded_expressions();
            Ok((fid, op_id))
        }
        PlanNodeEnum::Limit(limit) => {
            let spec = build_limit_spec(limit)?;
            ArenaPlanAssembler::push_global_unary_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                child_fid,
                op.id(),
                spec,
            )
        }
        PlanNodeEnum::Sort(sort) => {
            let spec = build_sort_spec(sort)?;
            let (fid, op_id) = ArenaPlanAssembler::push_global_blocking_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                child_fid,
                op.id(),
                spec,
                PhysicalProperties::single_blocking_spillable(SPILL_DEFAULT_THRESHOLD),
            )?;
            operators[op_id.0].has_folded_expressions = sort.has_folded_expressions();
            Ok((fid, op_id))
        }
        PlanNodeEnum::Aggregate(agg) => {
            let spec = build_aggregate_spec(agg)?;
            let (fid, op_id) = ArenaPlanAssembler::push_global_blocking_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                child_fid,
                op.id(),
                spec,
                PhysicalProperties::single_blocking_with_budget(),
            )?;
            operators[op_id.0].has_folded_expressions = agg.has_folded_expressions();
            Ok((fid, op_id))
        }
        PlanNodeEnum::TopN(topn) => {
            let spec = build_topn_spec(topn)?;
            ArenaPlanAssembler::push_global_blocking_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                child_fid,
                op.id(),
                spec,
                PhysicalProperties::single_blocking_with_budget(),
            )
        }
        PlanNodeEnum::Dedup(_) => ArenaPlanAssembler::push_global_blocking_op(
            operators,
            fragments,
            op_alloc,
            frag_alloc,
            child_fid,
            op.id(),
            BlockingSpec::Distinct,
            PhysicalProperties::single_blocking_with_budget(),
        ),
        PlanNodeEnum::Window(window) => {
            let spec = build_window_spec(window)?;
            let (fid, op_id) = ArenaPlanAssembler::push_global_blocking_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                child_fid,
                op.id(),
                spec,
                PhysicalProperties::single_blocking_with_budget(),
            )?;
            operators[op_id.0].has_folded_expressions = window.has_folded_expressions();
            Ok((fid, op_id))
        }
        _ => Err(PlanBuildError::unsupported(
            op.name(),
            op.id(),
            "operator is not supported above a partition exchange",
        )),
    }
}
