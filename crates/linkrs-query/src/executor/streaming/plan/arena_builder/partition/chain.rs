use super::super::super::super::operators::spec::{BlockingSpec, SourceSpec};
use super::super::super::properties::PhysicalProperties;
use super::super::super::types::{
    FragmentId, FragmentSpec, PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec,
};
use super::super::assembler::{ArenaFragmentAllocator, ArenaPlanAssembler, FragmentCtx};
use super::super::specs::{
    build_expand_all_spec_with_flags, build_filter_spec, build_flatten_spec, build_project_spec,
    build_source_spec, count_only_expand_below, is_count_only_aggregate, COUNT_ONLY_COLUMN,
};
use super::global::push_global_op;
use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;

/// Result of decomposing the logical root into a partitionable chain.
pub(super) struct PartitionedChain<'a> {
    /// The single tagged vertex scan at the bottom of the chain.
    pub(super) scan: &'a PlanNodeEnum,
    /// Filter/Project operators between the scan and the first global
    /// operator, ordered scan-first.  Copied into every partition fragment.
    pub(super) local: Vec<&'a PlanNodeEnum>,
    /// Operators from the first global operator up to the root, ordered
    /// scan-first.  Applied above the partition exchange.
    pub(super) global: Vec<&'a PlanNodeEnum>,
    /// The Aggregate node that should be split into partial+final phases,
    /// when it is the first global operator and all its functions support
    /// partial accumulation.
    pub(super) aggregate_split: Option<&'a AggregateNode>,
}
/// Build one partition group for a linear scan chain: one local fragment per
/// range (scan + local Filter/Project + optional PartialAggregate), a
/// Concatenate exchange, then the chain's global operators (including a
/// FinalAggregate for split aggregates). Returns the group root.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_chain_group(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    chain: &PartitionedChain,
    scan: &PlanNodeEnum,
    spec: &crate::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
    // 1. One local fragment per partition: StorageScanVertices bound to the
    //    partition's vertex-id range (or StorageScanEdges bound to a src-id
    //    range for edge scans), followed by the local Filter/Project pipeline
    //    and, for split aggregates, the PartialAggregate phase.
    let partition_fids = build_partition_local_fragments(
        operators, fragments, op_alloc, frag_alloc, chain, scan, spec, exec_ctx,
    )?;

    // 2. Exchange fragment: Concatenate over all partition fragments.
    let mut child_fid = ArenaPlanAssembler::push_exchange_op(
        operators,
        fragments,
        op_alloc,
        frag_alloc,
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
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
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
            operators, fragments, op_alloc, frag_alloc, child_fid, op, exec_ctx,
        )?
        .0;
    }

    let root_operator = fragments
        .get(child_fid.0)
        .map(|f| f.root_operator)
        .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "root fragment missing"))?;

    Ok((child_fid, root_operator))
}

/// Build one partition-local scan chain fragment per configured range: the
/// storage scan bound to that range plus the chain's local operators
/// (Filter/Project/ExpandAll) and, for split aggregates, the PartialAggregate
/// phase. Returns the fragment id per range in partition order.
///
/// Shared by the single-chain [`build_chain_group`] and the E1b co-partition
/// direct join, which pairs the left/right partition fragments by index.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_partition_local_fragments(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    chain: &PartitionedChain,
    scan: &PlanNodeEnum,
    spec: &crate::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> Result<Vec<FragmentId>, PlanBuildError> {
    let mut partition_fids = Vec::with_capacity(spec.partition_count());
    for range in spec.ranges() {
        let mut scan_spec = build_source_spec(scan, exec_ctx)?;
        match &mut scan_spec {
            SourceSpec::StorageScanVertices {
                partition_range, ..
            } => *partition_range = Some(range.clone()),
            SourceSpec::StorageScanEdges {
                partition_range, ..
            } => *partition_range = Some(range.clone()),
            _ => {
                return Err(PlanBuildError::unsupported(
                    "PhysicalPlan",
                    scan.id(),
                    "partitioned scan must lower to a storage vertex or edge scan",
                ));
            }
        }
        let (mut fid, _) = ArenaPlanAssembler::push_source_op(
            operators,
            fragments,
            op_alloc,
            frag_alloc,
            scan.id(),
            scan_spec,
        );
        for op in &chain.local {
            match op {
                PlanNodeEnum::Filter(filter) => {
                    let subquery_runners = super::super::assembler::build_subquery_runner_specs(
                        filter.subqueries(),
                        exec_ctx,
                    )?;
                    let spec = build_filter_spec(filter, subquery_runners)?;
                    let (new_fid, op_id) = ArenaPlanAssembler::push_unary_op(
                        operators,
                        fragments,
                        op_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?;
                    operators[op_id.0].has_folded_expressions = filter.has_folded_expressions();
                    fid = new_fid;
                }
                PlanNodeEnum::Project(project) => {
                    let subquery_runners = super::super::assembler::build_subquery_runner_specs(
                        project.subqueries(),
                        exec_ctx,
                    )?;
                    let spec = build_project_spec(project, subquery_runners)?;
                    let (new_fid, op_id) = ArenaPlanAssembler::push_unary_op(
                        operators,
                        fragments,
                        op_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?;
                    operators[op_id.0].has_folded_expressions = project.has_folded_expressions();
                    fid = new_fid;
                }
                PlanNodeEnum::ExpandAll(expand) => {
                    // A1.4: drive count_only from the node annotation so only
                    // the chain-tail expand skips row materialization; middle
                    // hops keep emitting raw destination ids for the next hop.
                    let spec =
                        build_expand_all_spec_with_flags(expand, exec_ctx, expand.count_only())?;
                    fid = ArenaPlanAssembler::push_graph_op(
                        operators,
                        fragments,
                        op_alloc,
                        frag_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?
                    .0;
                }
                PlanNodeEnum::Flatten(flatten) => {
                    let spec = build_flatten_spec(flatten)?;
                    let (new_fid, _) = ArenaPlanAssembler::push_unary_op(
                        operators,
                        fragments,
                        op_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?;
                    fid = new_fid;
                }
                _ => unreachable!("local chain holds filter/project/expand/flatten operators only"),
            }
        }
        if let Some(agg) = chain.aggregate_split {
            let (partial, _) = split_aggregate(agg);
            ArenaPlanAssembler::push_blocking_op(
                &mut FragmentCtx {
                    operators,
                    fragments,
                    op_alloc,
                },
                fid,
                agg.id(),
                partial,
                PhysicalProperties::single_blocking_with_budget(),
            )?;
        }
        partition_fids.push(fid);
    }
    Ok(partition_fids)
}
/// Decompose the logical root into a partitionable chain, or return `None`
/// when the tree is not a linear chain ending in a vertex scan.
pub(super) fn decompose(node: &PlanNodeEnum) -> Option<PartitionedChain<'_>> {
    let mut chain: Vec<&PlanNodeEnum> = Vec::new();
    if !collect_chain(node, &mut chain) {
        return None;
    }
    // chain is root-first, ending with the scan.
    let scan_index = chain.len() - 1;

    // Local operators: Filter/Project/ExpandAll directly above the scan, up
    // to the first global operator. ExpandAll must be the outermost local op
    // (its expansion is partition-local in E4 anchored traversals).
    let mut i = scan_index;
    let mut local: Vec<&PlanNodeEnum> = Vec::new();
    while i > 0 {
        let op = chain[i - 1];
        if matches!(
            op,
            PlanNodeEnum::Filter(_)
                | PlanNodeEnum::Project(_)
                | PlanNodeEnum::Flatten(_)
                | PlanNodeEnum::ExpandAll(_)
        ) {
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
/// an operator outside the supported set is encountered. An `ExpandAll` hop
/// is allowed (E4 anchored bounded traversal): it stays partition-local.
fn collect_chain<'a>(node: &'a PlanNodeEnum, chain: &mut Vec<&'a PlanNodeEnum>) -> bool {
    chain.push(node);
    match node {
        PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::ScanEdges(_) => true,
        PlanNodeEnum::Filter(filter) => collect_chain(filter.input(), chain),
        PlanNodeEnum::Project(project) => collect_chain(project.input(), chain),
        PlanNodeEnum::Flatten(flatten) => collect_chain(flatten.input(), chain),
        PlanNodeEnum::Limit(limit) => collect_chain(limit.input(), chain),
        PlanNodeEnum::Sort(sort) => collect_chain(sort.input(), chain),
        PlanNodeEnum::Aggregate(agg) => collect_chain(agg.input(), chain),
        PlanNodeEnum::TopN(topn) => collect_chain(topn.input(), chain),
        PlanNodeEnum::Dedup(dedup) => collect_chain(dedup.input(), chain),
        PlanNodeEnum::Window(window) => collect_chain(window.input(), chain),
        PlanNodeEnum::ExpandAll(expand) => {
            if let Some(input) = expand.inputs().first() {
                collect_chain(input, chain)
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Build the partial and final aggregate specs for a split aggregate node.
///
/// When the aggregate consumes a `count_only` expand (through its Project
/// pass-through), the `COUNT` functions are rewritten to `SUM(_expand_count)`
/// so the per-chunk edge counts are summed instead of counted as rows.
fn split_aggregate(node: &AggregateNode) -> (BlockingSpec, BlockingSpec) {
    let group_by_expressions: Vec<Expression> = node
        .group_keys()
        .iter()
        .map(|key| Expression::Variable(key.clone()))
        .collect();
    let count_only =
        is_count_only_aggregate(node) && count_only_expand_below(node.input()).is_some();
    let agg_args = node.aggregation_args();
    let aggregate_functions: Vec<(AggregateFunction, Vec<Expression>)> = node
        .aggregation_functions()
        .iter()
        .enumerate()
        .map(|(i, func)| {
            if count_only {
                (
                    AggregateFunction::Sum,
                    vec![Expression::Variable(COUNT_ONLY_COLUMN.to_string())],
                )
            } else {
                let args = agg_args.get(i).cloned().unwrap_or_default();
                (*func, args)
            }
        })
        .collect();
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
/// followed by a global merge (mirrors the predicate previously used by the
/// removed planner-side `PartitionedPhysicalPlan` decomposition).
fn all_functions_support_partial(funcs: &[AggregateFunction]) -> bool {
    funcs.iter().all(|f| {
        matches!(
            f,
            AggregateFunction::Count
                | AggregateFunction::Sum
                | AggregateFunction::Min
                | AggregateFunction::Max
                | AggregateFunction::Avg
        )
    })
}
