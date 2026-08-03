//! Partitioned physical-plan assembly (P8 wiring).
//!
//! When a [`PartitionSpec`] is present and the logical root is a linear
//! chain ending in a single tagged vertex scan, the builder duplicates the
//! partition-local pipeline once per range, gathers the partition outputs
//! through an Exchange fragment (`Concatenate`), and applies the global
//! operators on top of the exchange.
//!
//! Supported partition-local operators: Filter, Project.
//! Supported global operators: Filter, Project, Limit, Sort, Aggregate,
//! TopN, Dedup, Window.  Aggregate with partial-compatible functions
//! (COUNT/SUM/MIN/MAX/AVG) is split into a per-partition `PartialAggregate`
//! followed by a global `FinalAggregate`; all other shapes fall back to the
//! serial builder with a recorded reason.

use super::super::super::operators::spec::{BlockingSpec, SourceSpec};
use super::super::properties::{PhysicalProperties, SPILL_DEFAULT_THRESHOLD};
use super::super::types::{
    FragmentId, FragmentSpec, PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec,
};
use super::assembler::{ArenaFragmentAllocator, ArenaPlanAssembler, FragmentCtx};
use super::specs::{
    build_aggregate_spec, build_filter_spec, build_limit_spec, build_project_spec, build_sort_spec,
    build_source_spec, build_topn_spec, build_window_spec,
};
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::query::planning::plan::PartitionSource;

/// Result of decomposing the logical root into a partitionable chain.
struct PartitionedChain<'a> {
    /// The single tagged vertex scan at the bottom of the chain.
    scan: &'a PlanNodeEnum,
    /// Filter/Project operators between the scan and the first global
    /// operator, ordered scan-first.  Copied into every partition fragment.
    local: Vec<&'a PlanNodeEnum>,
    /// Operators from the first global operator up to the root, ordered
    /// scan-first.  Applied above the partition exchange.
    global: Vec<&'a PlanNodeEnum>,
    /// The Aggregate node that should be split into partial+final phases,
    /// when it is the first global operator and all its functions support
    /// partial accumulation.
    aggregate_split: Option<&'a AggregateNode>,
}

/// Try to build a partitioned physical plan from the logical root.
///
/// Returns `Ok(None)` when the plan shape is not partitionable (callers fall
/// back to the serial builder), `Ok(Some(...))` on success, and `Err` on
/// spec-conversion failures.
pub(super) fn build_partitioned(
    node: &PlanNodeEnum,
    spec: &crate::query::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> Result<
    Option<(
        Vec<PhysicalOperatorSpec>,
        Vec<FragmentSpec>,
        FragmentId,
        PhysicalOperatorId,
    )>,
    PlanBuildError,
> {
    let chain = match decompose(node) {
        Some(chain) => chain,
        None => return Ok(None),
    };
    let PartitionSource::VertexId { tag } = spec.source() else {
        return Ok(None);
    };
    let PlanNodeEnum::ScanVertices(scan_node) = chain.scan else {
        return Ok(None);
    };
    if scan_node.tag().map(|t| t.as_str()) != Some(tag.as_str()) {
        return Ok(None);
    }

    let mut operators = Vec::new();
    let mut fragments = Vec::new();
    let mut op_alloc = PhysicalOperatorIdAllocator::new();
    let mut frag_alloc = ArenaFragmentAllocator::new();

    // 1. One local fragment per partition: StorageScanVertices bound to the
    //    partition's vertex-id range, followed by the local Filter/Project
    //    pipeline and, for split aggregates, the PartialAggregate phase.
    let mut partition_fids = Vec::with_capacity(spec.partition_count());
    for range in spec.ranges() {
        let mut scan_spec = build_source_spec(chain.scan, exec_ctx)?;
        match &mut scan_spec {
            SourceSpec::StorageScanVertices {
                partition_range, ..
            } => *partition_range = Some(range.clone()),
            _ => {
                return Err(PlanBuildError::unsupported(
                    "PhysicalPlan",
                    scan_node.id(),
                    "partitioned scan must lower to a storage vertex scan",
                ));
            }
        }
        let (fid, _) = ArenaPlanAssembler::push_source_op(
            &mut operators,
            &mut fragments,
            &mut op_alloc,
            &mut frag_alloc,
            scan_node.id(),
            scan_spec,
        );
        for op in &chain.local {
            let spec = match op {
                PlanNodeEnum::Filter(filter) => build_filter_spec(filter)?,
                PlanNodeEnum::Project(project) => build_project_spec(project)?,
                _ => unreachable!("local chain holds filter/project operators only"),
            };
            ArenaPlanAssembler::push_unary_op(
                &mut operators,
                &mut fragments,
                &mut op_alloc,
                fid,
                op.id(),
                spec,
            )?;
        }
        if let Some(agg) = chain.aggregate_split {
            let (partial, _) = split_aggregate(agg);
            ArenaPlanAssembler::push_blocking_op(
                &mut FragmentCtx {
                    operators: &mut operators,
                    fragments: &mut fragments,
                    op_alloc: &mut op_alloc,
                },
                fid,
                agg.id(),
                partial,
                PhysicalProperties::single_blocking_with_budget(),
            )?;
        }
        partition_fids.push(fid);
    }

    // 2. Exchange fragment: Concatenate over all partition fragments.
    let mut child_fid = ArenaPlanAssembler::push_exchange_op(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        partition_fids,
        spec.partition_count(),
    )
    .0;

    // 3. Global operators above the exchange, scan-first order.
    for (index, op) in chain.global.iter().enumerate() {
        if index == 0 {
            if let Some(agg) = chain.aggregate_split {
                let (_, final_spec) = split_aggregate(agg);
                child_fid = ArenaPlanAssembler::push_global_blocking_op(
                    &mut operators,
                    &mut fragments,
                    &mut op_alloc,
                    &mut frag_alloc,
                    child_fid,
                    agg.id(),
                    final_spec,
                    PhysicalProperties::single_blocking_with_budget(),
                )?
                .0;
                continue;
            }
        }
        child_fid = push_global_op(
            &mut operators,
            &mut fragments,
            &mut op_alloc,
            &mut frag_alloc,
            child_fid,
            op,
        )?
        .0;
    }

    let root_fragment = child_fid;
    let root_operator = fragments
        .get(root_fragment.0)
        .map(|f| f.root_operator)
        .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "root fragment missing"))?;

    Ok(Some((operators, fragments, root_fragment, root_operator)))
}

/// Push one global operator as a new fragment consuming the current child.
fn push_global_op(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    child_fid: FragmentId,
    op: &PlanNodeEnum,
) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
    match op {
        PlanNodeEnum::Filter(filter) => {
            let spec = build_filter_spec(filter)?;
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
        PlanNodeEnum::Project(project) => {
            let spec = build_project_spec(project)?;
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
            ArenaPlanAssembler::push_global_blocking_op(
                operators,
                fragments,
                op_alloc,
                frag_alloc,
                child_fid,
                op.id(),
                spec,
                PhysicalProperties::single_blocking_spillable(SPILL_DEFAULT_THRESHOLD),
            )
        }
        PlanNodeEnum::Aggregate(agg) => {
            let spec = build_aggregate_spec(agg)?;
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
        _ => Err(PlanBuildError::unsupported(
            op.name(),
            op.id(),
            "operator is not supported above a partition exchange",
        )),
    }
}

/// Decompose the logical root into a partitionable chain, or return `None`
/// when the tree is not a linear chain ending in a vertex scan.
fn decompose(node: &PlanNodeEnum) -> Option<PartitionedChain<'_>> {
    let mut chain: Vec<&PlanNodeEnum> = Vec::new();
    if !collect_chain(node, &mut chain) {
        return None;
    }
    // chain is root-first, ending with the scan.
    let scan_index = chain.len() - 1;

    // Local operators: Filter/Project directly above the scan, up to the
    // first global operator.
    let mut i = scan_index;
    let mut local: Vec<&PlanNodeEnum> = Vec::new();
    while i > 0 {
        let op = chain[i - 1];
        if matches!(op, PlanNodeEnum::Filter(_) | PlanNodeEnum::Project(_)) {
            local.push(op);
            i -= 1;
        } else {
            break;
        }
    }

    // Global operators: the first global operator up to the root.
    let mut global: Vec<&PlanNodeEnum> = Vec::new();
    for j in (0..i).rev() {
        global.push(chain[j]);
    }

    let aggregate_split = match global.first() {
        Some(PlanNodeEnum::Aggregate(agg))
            if all_functions_support_partial(agg.aggregation_functions()) =>
        {
            Some(agg)
        }
        _ => None,
    };

    Some(PartitionedChain {
        scan: chain[scan_index],
        local,
        global,
        aggregate_split,
    })
}

/// Walk the linear chain from `node` down to its scan. Returns `false` when
/// an operator outside the supported set is encountered.
fn collect_chain<'a>(node: &'a PlanNodeEnum, chain: &mut Vec<&'a PlanNodeEnum>) -> bool {
    chain.push(node);
    match node {
        PlanNodeEnum::ScanVertices(_) => true,
        PlanNodeEnum::Filter(filter) => collect_chain(filter.input(), chain),
        PlanNodeEnum::Project(project) => collect_chain(project.input(), chain),
        PlanNodeEnum::Limit(limit) => collect_chain(limit.input(), chain),
        PlanNodeEnum::Sort(sort) => collect_chain(sort.input(), chain),
        PlanNodeEnum::Aggregate(agg) => collect_chain(agg.input(), chain),
        PlanNodeEnum::TopN(topn) => collect_chain(topn.input(), chain),
        PlanNodeEnum::Dedup(dedup) => collect_chain(dedup.input(), chain),
        PlanNodeEnum::Window(window) => collect_chain(window.input(), chain),
        _ => false,
    }
}

/// Build the partial and final aggregate specs for a split aggregate node.
fn split_aggregate(node: &AggregateNode) -> (BlockingSpec, BlockingSpec) {
    let group_by_expressions: Vec<Expression> = node
        .group_keys()
        .iter()
        .map(|key| Expression::Variable(key.clone()))
        .collect();
    let aggregate_functions = node.aggregation_functions().to_vec();
    let output_col_names = node.col_names().to_vec();
    (
        BlockingSpec::PartialAggregate {
            group_by_expressions: group_by_expressions.clone(),
            aggregate_functions: aggregate_functions.clone(),
            output_col_names: output_col_names.clone(),
        },
        BlockingSpec::FinalAggregate {
            group_by_expressions,
            aggregate_functions,
            output_col_names,
        },
    )
}

/// Aggregate functions that support per-partition partial accumulation
/// followed by a global merge (same predicate as
/// `PartitionedPhysicalPlan::split_node`).
fn all_functions_support_partial(funcs: &[AggregateFunction]) -> bool {
    funcs.iter().all(|f| {
        matches!(
            f,
            AggregateFunction::Count(_)
                | AggregateFunction::Sum(_)
                | AggregateFunction::Min(_)
                | AggregateFunction::Max(_)
                | AggregateFunction::Avg(_)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::expr::{ContextualExpression, ExpressionMeta};
    use crate::core::Expression;
    use crate::query::executor::base::ExecutionContext;
    use crate::query::executor::streaming::plan::arena_builder::PhysicalPlanBuilder;
    use crate::query::executor::streaming::plan::context::PhysicalPlanBuildContext;
    use crate::query::executor::streaming::plan::types::InputContract;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::query::planning::plan::core::nodes::ScanVerticesNode;
    use crate::query::planning::plan::{PartitionSource, PartitionSpec};
    use std::sync::Arc;

    fn test_context() -> ExecutionContext {
        ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()))
    }

    fn spec_with_two_ranges() -> PartitionSpec {
        PartitionSpec::try_new(
            vec![0..5, 5..10],
            PartitionSource::VertexId {
                tag: "person".to_string(),
            },
            None,
        )
        .expect("valid spec")
    }

    fn tagged_scan() -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "space");
        scan.set_tag("person");
        scan.set_col_names(vec!["v".to_string()]);
        PlanNodeEnum::ScanVertices(scan)
    }

    fn simple_filter(input: PlanNodeEnum) -> PlanNodeEnum {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Literal(crate::core::Value::Int(1));
        let id = expr_ctx.register_expression(ExpressionMeta::new(expr));
        let cond = ContextualExpression::new(id, expr_ctx);
        let filter = FilterNode::new(input, cond).expect("filter plan should build");
        PlanNodeEnum::Filter(filter)
    }

    #[test]
    fn partitionable_scan_chain_produces_partition_and_exchange_fragments() {
        let node = simple_filter(tagged_scan());
        let mut ctx = PhysicalPlanBuildContext::new();
        ctx.partition_spec = Some(spec_with_two_ranges());
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");

        // 2 partition fragments + 1 exchange fragment.
        assert_eq!(plan.fragment_count(), 3);
        assert_eq!(
            plan.root_fragment,
            crate::query::executor::streaming::plan::types::FragmentId(2)
        );

        // Every partition scan carries its own vertex-id range.
        let mut partition_ranges = Vec::new();
        for op in &plan.operators {
            if let crate::query::executor::streaming::plan::types::OperatorKindSpec::Source(
                SourceSpec::StorageScanVertices {
                    partition_range, ..
                },
            ) = &op.spec
            {
                partition_ranges.push(partition_range.clone());
            }
        }
        assert_eq!(partition_ranges, vec![Some(0..5), Some(5..10)]);

        // The exchange operator consumes both partitions through a
        // PartitionedInputs contract.
        let exchange = plan
            .operators
            .iter()
            .find(|op| {
                matches!(
                    op.spec,
                    crate::query::executor::streaming::plan::types::OperatorKindSpec::Exchange(_)
                )
            })
            .expect("exchange operator");
        match &exchange.input_contract {
            InputContract::PartitionedInputs { members, .. } => {
                assert_eq!(members.len(), 2);
                assert_eq!(members[0].partition_id, 0);
                assert_eq!(members[1].partition_id, 1);
            }
            other => panic!("expected PartitionedInputs, got {:?}", other),
        }

        crate::query::executor::streaming::plan::validator::PhysicalPlanValidator::validate(
            &Arc::new(plan),
        )
        .expect("partitioned plan should validate");
    }

    #[test]
    fn aggregate_is_split_into_partial_and_final_phases() {
        use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;

        let input = tagged_scan();
        let mut agg = AggregateNode::new(input, vec![], vec![AggregateFunction::Count(None)])
            .expect("aggregate plan should build");
        agg.set_col_names(vec!["count(*)".to_string()]);
        let node = PlanNodeEnum::Aggregate(agg);

        let mut ctx = PhysicalPlanBuildContext::new();
        ctx.partition_spec = Some(spec_with_two_ranges());
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");

        // 2 partition fragments + exchange + final aggregate fragment.
        assert_eq!(plan.fragment_count(), 4);

        let mut partial_count = 0;
        let mut final_count = 0;
        for op in &plan.operators {
            if let crate::query::executor::streaming::plan::types::OperatorKindSpec::Blocking(
                spec,
            ) = &op.spec
            {
                match spec {
                    BlockingSpec::PartialAggregate { .. } => partial_count += 1,
                    BlockingSpec::FinalAggregate { .. } => final_count += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(partial_count, 2);
        assert_eq!(final_count, 1);

        crate::query::executor::streaming::plan::validator::PhysicalPlanValidator::validate(
            &Arc::new(plan),
        )
        .expect("partitioned aggregate plan should validate");
    }

    #[test]
    fn unsupported_shape_falls_back_to_serial() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;

        let start = PlanNodeEnum::Start(StartNode::new());
        let mut ctx = PhysicalPlanBuildContext::new();
        ctx.partition_spec = Some(spec_with_two_ranges());
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&start, &mut ctx, &exec_ctx).expect("build");
        assert_eq!(plan.fragment_count(), 1);
        assert!(!ctx.parallel_fallback_reason.is_empty());
    }
}
