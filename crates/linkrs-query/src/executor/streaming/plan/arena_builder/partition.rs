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

use chain::{build_chain_group, decompose};
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
