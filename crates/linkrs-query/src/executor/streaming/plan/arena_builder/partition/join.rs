use super::super::super::super::operators::spec::{JoinSpec, SetSpec};
use super::super::super::types::PhysicalOperatorIdAllocator;
use super::super::assembler::{
    ArenaFragmentAllocator, ArenaPlanAssembler, BinaryOperatorSpec, FragmentCtx, HashExchangeParams,
};
use super::super::specs::build_inner_join_spec;
use super::chain::{
    build_chain_group, build_partition_local_fragments, decompose, PartitionedChain,
};
use super::PartitionBuildResult;
use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::{PartitionSource, PartitionStrategy};
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::Expression;

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
        PlanNodeEnum::Minus(minus) => Some((
            minus.input(),
            minus.minus_input(),
            IndependentBranchOp::Minus,
        )),
        PlanNodeEnum::Intersect(intersect) => Some((
            intersect.input(),
            intersect.intersect_input(),
            IndependentBranchOp::Intersect,
        )),
        PlanNodeEnum::CrossJoin(join) => Some((
            join.left_input(),
            join.right_input(),
            IndependentBranchOp::CrossJoin,
        )),
        PlanNodeEnum::InnerJoin(join) => {
            // E1b: allow equality join when the join key is a simple variable
            // reference (i.e. the vertex-id partition key).
            if equality_join_keys_are_simple(join.hash_keys(), join.probe_keys()) {
                Some((
                    join.left_input(),
                    join.right_input(),
                    IndependentBranchOp::InnerJoin,
                ))
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
        && hash_keys
            .first()
            .and_then(|k| k.expression())
            .is_some_and(|m| matches!(m.inner(), linkrs_core::types::expr::Expression::Variable(_)))
        && probe_keys
            .first()
            .and_then(|k| k.expression())
            .is_some_and(|m| matches!(m.inner(), linkrs_core::types::expr::Expression::Variable(_)))
}

/// Whether a join key references the vertex-id partition key (`vid`), the only
/// key that is partition-local under vertex-id range partitioning.
fn key_references_vid(expr: &ContextualExpression) -> bool {
    expr.expression().is_some_and(|meta| {
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
        _ => false,
    }
}

/// Build a partitioned plan for a set op / cross join over two independent
/// tagged vertex scan chains.
pub(super) fn build_partitioned_multi(
    node: &PlanNodeEnum,
    spec: &crate::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> PartitionBuildResult {
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

    // E1b hash exchange join: when join keys are simple variables but NOT the
    // vertex-id partition key, shuffle both sides by join key so matching rows
    // land in the same partition, then run partition-local joins.
    if let Some(join_spec) = join_spec.as_ref() {
        if let Some(result) = build_hash_exchange_join(
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
    spec: &crate::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> PartitionBuildResult {
    // Guards: the join key must be the vertex-id partition key and both
    // branches must be partition-local scan chains (no global operators or
    // split aggregates, which need a full gather before they can run).
    if !equality_join_keys_reference_vid(node)
        || !matches!(spec.strategy(), PartitionStrategy::Range)
    {
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

/// Build a hash-exchange join (E1b): shuffle both sides by join key, then
/// N partition-local joins, followed by a Concatenate exchange.
///
/// This path is used when the join key is a simple variable reference but
/// NOT the vertex-id partition key. The hash exchange redistributes rows
/// by the join key so that matching rows land in the same partition,
/// enabling per-partition joins.
fn build_hash_exchange_join(
    node: &PlanNodeEnum,
    join_spec: JoinSpec,
    left_chain: &PartitionedChain,
    right_chain: &PartitionedChain,
    spec: &crate::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> PartitionBuildResult {
    // Guards: join keys must be simple variables, and both branches must be
    // partition-local scan chains (no global operators or split aggregates).
    // Range layouts slice the scan input by id ranges; Hash layouts declare a
    // hash(key) distribution contract on top of the same disjoint slices.
    if !equality_join_keys_are_simple(hash_keys_of(node), probe_keys_of(node)) {
        return Ok(None);
    }
    match spec.strategy() {
        PartitionStrategy::Range | PartitionStrategy::Hash { .. } => {}
        PartitionStrategy::RoundRobin => return Ok(None),
    }
    if equality_join_keys_reference_vid(node) {
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

    // Build partition-local fragments for both sides.
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

    // Extract hash expressions from join keys.
    let hash_exprs = extract_join_key_expressions(node);
    if hash_exprs.is_empty() {
        return Ok(None);
    }

    // Create hash exchange operators for both sides to shuffle by join key.
    let (left_hash_fid, _) = ArenaPlanAssembler::push_hash_exchange_op(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        HashExchangeParams {
            partition_fids: left_frags,
            partition_count: spec.partition_count(),
            hash_expressions: hash_exprs.clone(),
            input_layout: None,
            output_layout: None,
        },
    );
    let (right_hash_fid, _) = ArenaPlanAssembler::push_hash_exchange_op(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        HashExchangeParams {
            partition_fids: right_frags,
            partition_count: spec.partition_count(),
            hash_expressions: hash_exprs,
            input_layout: None,
            output_layout: None,
        },
    );

    // Create N partition-local joins. Each join receives one partition from
    // each hash exchange output.
    let mut join_fids = Vec::with_capacity(spec.partition_count());
    for _ in 0..spec.partition_count() {
        let (fid, _) = ArenaPlanAssembler::push_binary_op(
            &mut FragmentCtx {
                operators: &mut operators,
                fragments: &mut fragments,
                op_alloc: &mut op_alloc,
            },
            &mut frag_alloc,
            left_hash_fid,
            right_hash_fid,
            node.id(),
            join_spec.clone(),
        )?;
        join_fids.push(fid);
    }

    // Gather partition-local join results.
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

/// Extract hash key expressions from a join node.
fn hash_keys_of(node: &PlanNodeEnum) -> &[ContextualExpression] {
    match node {
        PlanNodeEnum::InnerJoin(join) => join.hash_keys(),
        _ => &[],
    }
}

/// Extract probe key expressions from a join node.
fn probe_keys_of(node: &PlanNodeEnum) -> &[ContextualExpression] {
    match node {
        PlanNodeEnum::InnerJoin(join) => join.probe_keys(),
        _ => &[],
    }
}

/// Extract the hash key expressions as plain `Expression` values from a join node.
fn extract_join_key_expressions(node: &PlanNodeEnum) -> Vec<Expression> {
    match node {
        PlanNodeEnum::InnerJoin(join) => join
            .hash_keys()
            .iter()
            .filter_map(|k| k.get_expression())
            .collect(),
        _ => Vec::new(),
    }
}
