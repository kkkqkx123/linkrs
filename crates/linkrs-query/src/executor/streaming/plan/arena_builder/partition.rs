use super::super::types::{
    FragmentId, FragmentSpec, PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec,
};
use super::assembler::ArenaFragmentAllocator;
use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::{PartitionSource, PartitionStrategy};

mod chain;
mod global;
mod join;
#[cfg(test)]
mod tests;

pub(super) use chain::local_topk_decision;
use chain::{build_chain_group, build_ordered_existence_group, decompose};
use join::build_partitioned_multi;

/// A partitioned physical plan: operators, fragments, root fragment, root operator.
pub(super) type PartitionedPlan = (
    Vec<PhysicalOperatorSpec>,
    Vec<FragmentSpec>,
    FragmentId,
    PhysicalOperatorId,
);

/// Result of building a partitioned plan.
pub(super) type PartitionBuildResult = Result<Option<PartitionedPlan>, PlanBuildError>;

/// Build the ordered existence plan independently of partition configuration.
/// The rewrite is safe from a single ordered source, so it does not need a
/// partition spec and is not affected by worker counts.
pub(super) fn build_ordered_existence(
    node: &PlanNodeEnum,
    exec_ctx: &ExecutionContext,
) -> PartitionBuildResult {
    let Some(chain) = decompose(node) else {
        return Ok(None);
    };
    let Ok(Some(proof)) = chain::ordered_existence_scan(&chain, exec_ctx) else {
        return Ok(None);
    };
    let mut operators = Vec::new();
    let mut fragments = Vec::new();
    let mut op_alloc = PhysicalOperatorIdAllocator::new();
    let mut frag_alloc = ArenaFragmentAllocator::new();
    let (root_fragment, root_operator) = build_ordered_existence_group(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        &chain,
        &proof,
        exec_ctx,
    )?;
    Ok(Some((operators, fragments, root_fragment, root_operator)))
}

/// Diagnose the ordered existence rewrite for EXPLAIN. Emits a note only for
/// the narrow query shape it is meant to serve.
pub(super) fn ordered_existence_decision(
    node: &PlanNodeEnum,
    exec_ctx: &ExecutionContext,
    redundant_ops_removed: bool,
) -> Option<String> {
    let chain = decompose(node)?;
    match chain::ordered_existence_scan(&chain, exec_ctx) {
        Ok(Some(proof)) => {
            let mut note = format!(
                "ordered_existence:proven,reason={},limit={},index={}",
                proof.reason, proof.limit, proof.index_name
            );
            if redundant_ops_removed {
                note.push_str(";redundant_global_ops=eliminated");
            }
            Some(note)
        }
        Ok(None) => None,
        Err(reason) => Some(format!("ordered_existence:unproven,reason={reason}")),
    }
}

/// Try to build a partitioned physical plan from the logical root.
///
/// Returns `Ok(None)` when the plan shape is not partitionable (callers fall
/// back to the serial builder), `Ok(Some(...))` on success, and `Err` on
/// spec-conversion failures.
pub(super) fn build_partitioned(
    node: &PlanNodeEnum,
    spec: &crate::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> PartitionBuildResult {
    // Multi-branch: a set op or cross join over two independent scan chains,
    // each partitioned with the shared ranges and gathered before the global
    // binary operator.
    if let Some(result) = build_partitioned_multi(node, spec, exec_ctx)? {
        return Ok(Some(result));
    }

    // Linear chains must be range-sliced: hash/round-robin layouts are only
    // consumed by the multi-branch hash-exchange join above. Falling back
    // here lets the serial builder run with a recorded reason.
    if !matches!(spec.strategy(), PartitionStrategy::Range) {
        return Ok(None);
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
