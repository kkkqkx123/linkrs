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

use super::super::super::operators::spec::{BlockingSpec, JoinSpec, SetSpec, SourceSpec};
use super::super::properties::{PhysicalProperties, SPILL_DEFAULT_THRESHOLD};
use super::super::types::{
    FragmentId, FragmentSpec, PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec,
};
use super::assembler::{
    ArenaFragmentAllocator, ArenaPlanAssembler, BinaryOperatorSpec, FragmentCtx,
};
use super::specs::{
    build_aggregate_spec, build_expand_all_spec_with_flags, build_filter_spec,
    build_hash_inner_join_spec, build_inner_join_spec, build_limit_spec, build_project_spec,
    build_sort_spec, build_source_spec, build_topn_spec, build_window_spec, count_only_expand_below,
    is_count_only_aggregate, COUNT_ONLY_COLUMN,
};
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
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
    // Multi-branch: a set op or cross join over two independent scan chains,
    // each partitioned with the shared ranges and gathered before the global
    // binary operator.
    if let Some(result) = build_partitioned_multi(node, spec, exec_ctx)? {
        return Ok(Some(result));
    }

    let chain = match decompose(node) {
        Some(chain) => chain,
        None => return Ok(None),
    };
    let scan = match spec.source() {
        PartitionSource::VertexId { tag } => {
            let PlanNodeEnum::ScanVertices(scan_node) = chain.scan else {
                return Ok(None);
            };
            if scan_node.tag().map(|t| t.as_str()) != Some(tag.as_str()) {
                return Ok(None);
            }
            chain.scan
        }
        PartitionSource::EdgeId { edge_type } => {
            let PlanNodeEnum::ScanEdges(scan_node) = chain.scan else {
                return Ok(None);
            };
            if scan_node.edge_type().as_deref() != Some(edge_type.as_str()) {
                return Ok(None);
            }
            chain.scan
        }
        PartitionSource::Index { .. } => return Ok(None),
    };

    let mut operators = Vec::new();
    let mut fragments = Vec::new();
    let mut op_alloc = PhysicalOperatorIdAllocator::new();
    let mut frag_alloc = ArenaFragmentAllocator::new();

    let (root_fragment, root_operator) = build_chain_group(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        &chain,
        scan,
        spec,
        exec_ctx,
    )?;

    Ok(Some((operators, fragments, root_fragment, root_operator)))
}

/// Build one partition group for a linear scan chain: one local fragment per
/// range (scan + local Filter/Project + optional PartialAggregate), a
/// Concatenate exchange, then the chain's global operators (including a
/// FinalAggregate for split aggregates). Returns the group root.
#[allow(clippy::too_many_arguments)]
fn build_chain_group(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    chain: &PartitionedChain,
    scan: &PlanNodeEnum,
    spec: &crate::query::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
    // 1. One local fragment per partition: StorageScanVertices bound to the
    //    partition's vertex-id range (or StorageScanEdges bound to a src-id
    //    range for edge scans), followed by the local Filter/Project pipeline
    //    and, for split aggregates, the PartialAggregate phase.
    let partition_fids = build_partition_local_fragments(
        operators,
        fragments,
        op_alloc,
        frag_alloc,
        chain,
        scan,
        spec,
        exec_ctx,
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
            operators,
            fragments,
            op_alloc,
            frag_alloc,
            child_fid,
            op,
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
fn build_partition_local_fragments(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    chain: &PartitionedChain,
    scan: &PlanNodeEnum,
    spec: &crate::query::planning::plan::PartitionSpec,
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
                    let spec = build_filter_spec(filter)?;
                    fid = ArenaPlanAssembler::push_unary_op(
                        operators,
                        fragments,
                        op_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?
                    .0;
                }
                PlanNodeEnum::Project(project) => {
                    let spec = build_project_spec(project)?;
                    fid = ArenaPlanAssembler::push_unary_op(
                        operators,
                        fragments,
                        op_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?
                    .0;
                }
                PlanNodeEnum::ExpandAll(expand) => {
                    // A1.4: drive count_only from the node annotation so only
                    // the chain-tail expand skips row materialization; middle
                    // hops keep emitting raw destination ids for the next hop.
                    let spec = build_expand_all_spec_with_flags(expand, exec_ctx, expand.count_only())?;
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
                _ => unreachable!("local chain holds filter/project/expand operators only"),
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

/// The binary operators over independent scan branches that E1a/E1b can partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndependentBranchOp {
    Union,
    UnionAll,
    Minus,
    Intersect,
    CrossJoin,
    /// E1b: equality join on the partition key (vertex-id).
    InnerJoin,
}

/// Split a binary-op root into two independent branch inputs.  Equality joins
/// on a simple variable key (vertex-id) are supported by E1b.
fn split_independent_branches(
    node: &PlanNodeEnum,
) -> Option<(&PlanNodeEnum, &PlanNodeEnum, IndependentBranchOp)> {
    match node {
        PlanNodeEnum::Union(union) => {
            let op = if union.distinct() {
                IndependentBranchOp::Union
            } else {
                IndependentBranchOp::UnionAll
            };
            Some((union.input(), union.union_input(), op))
        }
        PlanNodeEnum::Minus(minus) => Some((minus.input(), minus.minus_input(), IndependentBranchOp::Minus)),
        PlanNodeEnum::Intersect(intersect) => {
            Some((intersect.input(), intersect.intersect_input(), IndependentBranchOp::Intersect))
        }
        PlanNodeEnum::CrossJoin(join) => {
            Some((join.left_input(), join.right_input(), IndependentBranchOp::CrossJoin))
        }
        PlanNodeEnum::InnerJoin(join) => {
            // E1b: allow equality join when the join key is a simple variable
            // reference (i.e. the vertex-id partition key).
            if equality_join_keys_are_simple(join.hash_keys(), join.probe_keys()) {
                Some((join.left_input(), join.right_input(), IndependentBranchOp::InnerJoin))
            } else {
                None
            }
        }
        PlanNodeEnum::HashInnerJoin(join) => {
            // E1b: real keyed-join queries lower to a HashInnerJoin node; it is
            // partitioned the same way as the plain InnerJoin variant.
            if equality_join_keys_are_simple(join.hash_keys(), join.probe_keys()) {
                Some((join.left_input(), join.right_input(), IndependentBranchOp::InnerJoin))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Whether a join's hash/probe keys are each a single simple variable
/// reference (the only key shape the partitioned join path supports).
fn equality_join_keys_are_simple(
    hash_keys: &[ContextualExpression],
    probe_keys: &[ContextualExpression],
) -> bool {
    hash_keys.len() == 1
        && probe_keys.len() == 1
        && hash_keys.first().and_then(|k| k.expression()).map_or(false, |m| {
            matches!(m.inner(), crate::core::types::expr::Expression::Variable(_))
        })
        && probe_keys.first().and_then(|k| k.expression()).map_or(false, |m| {
            matches!(m.inner(), crate::core::types::expr::Expression::Variable(_))
        })
}

/// Whether a join key references the vertex-id partition key (`vid`), the only
/// key that is partition-local under vertex-id range partitioning.
fn key_references_vid(expr: &ContextualExpression) -> bool {
    expr.expression().map_or(false, |meta| {
        matches!(
            meta.inner(),
            Expression::Variable(name) if name == "vid" || name.ends_with(".vid")
        )
    })
}

/// Whether every hash/probe key of the join references the vertex-id partition
/// key. Co-partition direct join is only correct for such keys: two matching
/// rows must land in the same vertex-id range partition.
fn equality_join_keys_reference_vid(node: &PlanNodeEnum) -> bool {
    match node {
        PlanNodeEnum::InnerJoin(join) => {
            !join.hash_keys().is_empty()
                && join.hash_keys().iter().all(key_references_vid)
                && !join.probe_keys().is_empty()
                && join.probe_keys().iter().all(key_references_vid)
        }
        PlanNodeEnum::HashInnerJoin(join) => {
            !join.hash_keys().is_empty()
                && join.hash_keys().iter().all(key_references_vid)
                && !join.probe_keys().is_empty()
                && join.probe_keys().iter().all(key_references_vid)
        }
        _ => false,
    }
}

/// Build a partitioned plan for a set op / cross join over two independent
/// tagged vertex scan chains.
fn build_partitioned_multi(
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
    let Some((left, right, op)) = split_independent_branches(node) else {
        return Ok(None);
    };
    let PartitionSource::VertexId { .. } = spec.source() else {
        return Ok(None);
    };
    let Some(left_chain) = decompose(left) else {
        return Ok(None);
    };
    let Some(right_chain) = decompose(right) else {
        return Ok(None);
    };
    // Multi-scan partitioning covers vertex scans only.
    let (PlanNodeEnum::ScanVertices(_), PlanNodeEnum::ScanVertices(_)) =
        (left_chain.scan, right_chain.scan)
    else {
        return Ok(None);
    };

    // E1b: carry the equality condition from the logical join keys. Dropping
    // it would turn a partitioned equality join into an unconditional cross
    // join (nested-loop join matches every left x right pair when the
    // condition is None).
    let join_spec: Option<JoinSpec> = match node {
        PlanNodeEnum::InnerJoin(join) => Some(build_inner_join_spec(join)?),
        PlanNodeEnum::HashInnerJoin(join) => Some(build_hash_inner_join_spec(join)?),
        _ => None,
    };

    // E1b co-partition direct join: when both sides are simple scan chains
    // (no global operators / aggregates) and the join key is the vertex-id
    // partition key, pair the partition-local scan fragments and join them
    // per-partition before the global exchange. Guards that fail fall back to
    // the global join path below.
    if let Some(join_spec) = join_spec.as_ref() {
        if let Some(result) = build_co_partitioned_join(
            node,
            join_spec.clone(),
            &left_chain,
            &right_chain,
            spec,
            exec_ctx,
        )? {
            return Ok(Some(result));
        }
    }

    let mut operators = Vec::new();
    let mut fragments = Vec::new();
    let mut op_alloc = PhysicalOperatorIdAllocator::new();
    let mut frag_alloc = ArenaFragmentAllocator::new();

    let (left_fid, _) = build_chain_group(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        &left_chain,
        left_chain.scan,
        spec,
        exec_ctx,
    )?;
    let (right_fid, _) = build_chain_group(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        &right_chain,
        right_chain.scan,
        spec,
        exec_ctx,
    )?;

    let binary_spec: BinaryOperatorSpec = match op {
            IndependentBranchOp::Union => SetSpec::Union.into(),
            IndependentBranchOp::UnionAll => SetSpec::UnionAll.into(),
            IndependentBranchOp::Minus => SetSpec::Minus.into(),
            IndependentBranchOp::Intersect => SetSpec::Intersect.into(),
            IndependentBranchOp::CrossJoin => JoinSpec::CrossJoin.into(),
            IndependentBranchOp::InnerJoin => join_spec
                .ok_or_else(|| {
                    PlanBuildError::unsupported(
                        "PhysicalPlan",
                        node.id(),
                        "partitioned equality join is missing its join condition",
                    )
                })?
                .into(),
        };
    let (root_fid, root_op) = ArenaPlanAssembler::push_binary_op(
        &mut FragmentCtx {
            operators: &mut operators,
            fragments: &mut fragments,
            op_alloc: &mut op_alloc,
        },
        &mut frag_alloc,
        left_fid,
        right_fid,
        node.id(),
        binary_spec,
    )?;

    Ok(Some((operators, fragments, root_fid, root_op)))
}

/// Build a co-partitioned direct join (E1b): N partition-local joins, one per
/// vertex-id range, followed by a Concatenate exchange.
///
/// This is only correct when the join key is the vertex-id partition key:
/// matching rows then carry the same vid and land in the same range partition,
/// so the per-partition join emits exactly the global join result. Anything
/// else (non-vid keys, branches with global operators or split aggregates)
/// must use the global gather-then-join path instead.
#[allow(clippy::too_many_arguments)]
fn build_co_partitioned_join(
    node: &PlanNodeEnum,
    join_spec: JoinSpec,
    left_chain: &PartitionedChain,
    right_chain: &PartitionedChain,
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
    // Guards: the join key must be the vertex-id partition key and both
    // branches must be partition-local scan chains (no global operators or
    // split aggregates, which need a full gather before they can run).
    if !equality_join_keys_reference_vid(node) {
        return Ok(None);
    }
    if !left_chain.global.is_empty()
        || !right_chain.global.is_empty()
        || left_chain.aggregate_split.is_some()
        || right_chain.aggregate_split.is_some()
    {
        return Ok(None);
    }

    let mut operators = Vec::new();
    let mut fragments = Vec::new();
    let mut op_alloc = PhysicalOperatorIdAllocator::new();
    let mut frag_alloc = ArenaFragmentAllocator::new();

    let left_frags = build_partition_local_fragments(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        left_chain,
        left_chain.scan,
        spec,
        exec_ctx,
    )?;
    let right_frags = build_partition_local_fragments(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        right_chain,
        right_chain.scan,
        spec,
        exec_ctx,
    )?;

    // One local join per range: left partition i joined with right partition i
    // over the shared vertex-id range, so the exchange can run them in
    // parallel on the worker pool.
    let mut join_fids = Vec::with_capacity(spec.partition_count());
    for index in 0..spec.partition_count() {
        let (fid, _) = ArenaPlanAssembler::push_binary_op(
            &mut FragmentCtx {
                operators: &mut operators,
                fragments: &mut fragments,
                op_alloc: &mut op_alloc,
            },
            &mut frag_alloc,
            left_frags[index],
            right_frags[index],
            node.id(),
            join_spec.clone(),
        )?;
        join_fids.push(fid);
    }

    let (root_fid, _) = ArenaPlanAssembler::push_exchange_op(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        join_fids,
        spec.partition_count(),
    );
    let root_operator = fragments
        .get(root_fid.0)
        .map(|f| f.root_operator)
        .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "root fragment missing"))?;

    Ok(Some((operators, fragments, root_fid, root_operator)))
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
    let count_only = is_count_only_aggregate(node)
        && count_only_expand_below(node.input()).is_some();
    let aggregate_functions: Vec<AggregateFunction> = node
        .aggregation_functions()
        .iter()
        .map(|func| {
            if count_only {
                AggregateFunction::Sum(COUNT_ONLY_COLUMN.to_string())
            } else {
                func.clone()
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

    /// Build a keyed equality join plan node with the given hash/probe key
    /// variable names over two tagged scans.
    fn keyed_join(
        hash_key_name: &str,
        probe_key_name: &str,
    ) -> PlanNodeEnum {
        use crate::query::planning::plan::core::nodes::join::join_node::InnerJoinNode;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let make_key = |name: &str| {
            let expr = Expression::Variable(name.to_string());
            let id = expr_ctx.register_expression(ExpressionMeta::new(expr));
            ContextualExpression::new(id, expr_ctx.clone())
        };
        let mut left_scan = ScanVerticesNode::new(1, "space");
        left_scan.set_tag("person");
        left_scan.set_col_names(vec!["a".to_string()]);
        let mut right_scan = ScanVerticesNode::new(2, "space");
        right_scan.set_tag("person");
        right_scan.set_col_names(vec!["b".to_string()]);
        PlanNodeEnum::InnerJoin(
            InnerJoinNode::new(
                PlanNodeEnum::ScanVertices(left_scan),
                PlanNodeEnum::ScanVertices(right_scan),
                vec![make_key(hash_key_name)],
                vec![make_key(probe_key_name)],
            )
            .expect("join plan should build"),
        )
    }

    #[test]
    fn co_partitioned_equality_join_pairs_partition_fragments() {
        // A vid-key equality join over two simple scan chains takes the
        // co-partition direct-join path: one local join per vertex-id range,
        // gathered through a single Concatenate exchange.
        let node = keyed_join("a.vid", "b.vid");
        let mut ctx = PhysicalPlanBuildContext::new();
        ctx.partition_spec = Some(spec_with_two_ranges());
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");
        assert!(
            ctx.parallel_fallback_reason.is_empty(),
            "vid-key join must not fall back, got: {}",
            ctx.parallel_fallback_reason
        );

        // 2 left scans + 2 right scans + 2 local joins + 1 exchange.
        assert_eq!(plan.fragment_count(), 7);
        assert_eq!(
            plan.fragments
                .fragments()
                .iter()
                .filter(|f| matches!(
                    f.kind,
                    crate::query::executor::streaming::plan::types::FragmentKind::Exchange
                ))
                .count(),
            1,
            "co-partitioned join must gather through exactly one exchange"
        );

        // One local join per partition, each carrying the equality condition.
        let mut join_count = 0;
        for op in &plan.operators {
            if let crate::query::executor::streaming::plan::types::OperatorKindSpec::Join(
                JoinSpec::InnerJoin { join_condition },
            ) = &op.spec
            {
                join_count += 1;
                assert!(
                    join_condition.is_some(),
                    "partitioned equality join must carry its key condition"
                );
            }
        }
        assert_eq!(join_count, 2, "one local join per partition");

        // The exchange consumes both per-partition joins.
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
        .expect("co-partitioned join plan should validate");
    }

    #[test]
    fn non_vid_equality_join_falls_back_to_global_gather_join() {
        // A join on a non-vid key is not partition-local: it must fall back to
        // the global gather-then-join path while STILL carrying the equality
        // condition (R1 regression: the condition must not be dropped).
        let node = keyed_join("a.value", "b.value");
        let mut ctx = PhysicalPlanBuildContext::new();
        ctx.partition_spec = Some(spec_with_two_ranges());
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");
        assert!(
            ctx.parallel_fallback_reason.is_empty(),
            "non-vid equality join must still partition via the global path, got: {}",
            ctx.parallel_fallback_reason
        );

        // Each branch is a full chain group (2 scans + 1 exchange each), then
        // a single global join over both exchanges.
        assert_eq!(plan.fragment_count(), 7);
        assert_eq!(
            plan.fragments
                .fragments()
                .iter()
                .filter(|f| matches!(
                    f.kind,
                    crate::query::executor::streaming::plan::types::FragmentKind::Exchange
                ))
                .count(),
            2,
            "global join path gathers each branch separately"
        );

        let mut join_count = 0;
        for op in &plan.operators {
            if let crate::query::executor::streaming::plan::types::OperatorKindSpec::Join(
                JoinSpec::InnerJoin { join_condition },
            ) = &op.spec
            {
                join_count += 1;
                assert!(
                    join_condition.is_some(),
                    "global equality join must carry its key condition"
                );
            }
        }
        assert_eq!(join_count, 1, "exactly one global join fragment");

        crate::query::executor::streaming::plan::validator::PhysicalPlanValidator::validate(
            &Arc::new(plan),
        )
        .expect("global-gather join plan should validate");
    }
}
