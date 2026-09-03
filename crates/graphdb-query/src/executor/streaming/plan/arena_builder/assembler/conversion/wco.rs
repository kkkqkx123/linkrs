use super::super::super::super::types::{FragmentId, PhysicalOperatorId};
use super::super::super::specs::*;
use super::super::fragment_ops::FragmentCtx;
use super::super::{ArenaFragmentAllocator, ArenaPlanAssembler};
use crate::executor::build_error::PlanBuildError;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::join::wco_intersect_node::WcoIntersectNode;

impl ArenaPlanAssembler {
    pub(crate) fn convert_wco_node(
        node: &WcoIntersectNode,
        operators: &mut Vec<super::super::super::super::types::PhysicalOperatorSpec>,
        fragments: &mut Vec<super::super::super::super::types::FragmentSpec>,
        op_alloc: &mut super::super::super::super::types::PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        exec_ctx: &crate::executor::base::ExecutionContext,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let mut input_fids = Vec::with_capacity(node.num_builds() + 1);
        for child in node.dependencies() {
            let (fid, _) = Self::convert_node(
                child, operators, fragments, op_alloc, frag_alloc, exec_ctx,
            )?;
            input_fids.push(fid);
        }
        let spec = build_wco_spec(node)?;
        Self::push_nary_wco_op(
            &mut FragmentCtx {
                operators,
                fragments,
                op_alloc,
            },
            frag_alloc,
            input_fids,
            node.id(),
            spec,
        )
    }
}
