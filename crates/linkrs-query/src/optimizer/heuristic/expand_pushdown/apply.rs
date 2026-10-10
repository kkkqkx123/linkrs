use std::collections::HashMap;

use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    BinaryInputNode, MultipleInputNode, PlanNode, SingleInputNode,
};

use super::ExpandDecision;

/// Apply the flag decisions to the matching `ExpandAll` nodes in place.
pub(super) fn apply_decisions(
    root: &mut PlanNodeEnum,
    decisions: &HashMap<i64, ExpandDecision>,
) -> bool {
    let mut changed = false;
    if let PlanNodeEnum::ExpandAll(expand) = root {
        if let Some(decision) = decisions.get(&expand.id()) {
            if expand.id_only() != decision.id_only
                || expand.count_only() != decision.count_only
                || expand.lightweight_source() != decision.lightweight_source
                || expand.edge_required_props() != decision.edge_props.as_ref()
                || expand.dst_required_props() != decision.dst_props.as_ref()
                || expand.closed_loop() != decision.closed_loop
                || expand.skip_rows() != decision.skip_rows
            {
                expand.set_id_only(decision.id_only);
                expand.set_count_only(decision.count_only);
                expand.set_lightweight_source(decision.lightweight_source);
                expand.set_edge_required_props(decision.edge_props.clone());
                expand.set_dst_required_props(decision.dst_props.clone());
                expand.set_closed_loop(decision.closed_loop);
                expand.set_skip_rows(decision.skip_rows);
                changed = true;
            }
        }
    }
    use PlanNodeEnum::*;
    match root {
        Project(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Filter(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Flatten(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Sort(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Limit(n) => changed |= apply_decisions(n.input_mut(), decisions),
        TopN(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Sample(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Dedup(n) => changed |= apply_decisions(n.input_mut(), decisions),
        DataCollect(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Aggregate(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Window(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Unwind(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Assign(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Remove(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Materialize(n) => changed |= apply_decisions(n.input_mut(), decisions),
        PatternApply(n) => changed |= apply_decisions(n.input_mut(), decisions),
        CorrelatedApply(n) => changed |= apply_decisions(n.input_mut(), decisions),
        RollUpApply(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Traverse(n) => changed |= apply_decisions(n.input_mut(), decisions),
        PipeDeleteVertices(n) => changed |= apply_decisions(n.input_mut(), decisions),
        PipeDeleteEdges(n) => changed |= apply_decisions(n.input_mut(), decisions),
        Expand(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        ExpandAll(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        AppendVertices(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        GetVertices(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        GetNeighbors(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        InnerJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        LeftJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        RightJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        CrossJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        FullOuterJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        SemiJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        Apply(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        BiExpand(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        BiTraverse(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions);
            changed |= apply_decisions(n.right_input_mut(), decisions);
        }
        Union(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        Minus(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        Intersect(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions);
            }
        }
        _ => {}
    }
    changed
}
