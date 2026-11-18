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
    rowless_projects: &std::collections::HashSet<i64>,
) -> bool {
    let mut changed = false;
    if let PlanNodeEnum::ExpandAll(expand) = root {
        if let Some(decision) = decisions.get(&expand.id()) {
            if expand.id_only() != decision.id_only
                || expand.count_only() != decision.count_only
                || expand.lightweight_source() != decision.lightweight_source
                || expand.edge_required_props() != decision.edge_props.as_deref()
                || expand.dst_required_props() != decision.dst_props.as_deref()
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
        Project(n) => {
            if rowless_projects.contains(&n.id()) && !n.emit_rowless() {
                n.set_emit_rowless(true);
                changed = true;
            }
            changed |= apply_decisions(n.input_mut(), decisions, rowless_projects);
        }
        Filter(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Flatten(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Sort(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Limit(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        TopN(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Sample(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Dedup(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        DataCollect(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Aggregate(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Window(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Unwind(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Assign(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Materialize(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        PatternApply(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        CorrelatedApply(n) => {
            changed |= apply_decisions(n.input_mut(), decisions, rowless_projects)
        }
        RollUpApply(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        Traverse(n) => changed |= apply_decisions(n.input_mut(), decisions, rowless_projects),
        PipeDeleteVertices(n) => {
            changed |= apply_decisions(n.input_mut(), decisions, rowless_projects)
        }
        PipeDeleteEdges(n) => {
            changed |= apply_decisions(n.input_mut(), decisions, rowless_projects)
        }
        Expand(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions, rowless_projects);
            }
        }
        ExpandAll(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions, rowless_projects);
            }
        }
        AppendVertices(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions, rowless_projects);
            }
        }
        GetVertices(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions, rowless_projects);
            }
        }
        GetNeighbors(n) => {
            for child in n.inputs_mut() {
                changed |= apply_decisions(child, decisions, rowless_projects);
            }
        }
        InnerJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        LeftJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        RightJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        CrossJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        FullOuterJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        SemiJoin(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        Apply(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        BiExpand(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        BiTraverse(n) => {
            changed |= apply_decisions(n.left_input_mut(), decisions, rowless_projects);
            changed |= apply_decisions(n.right_input_mut(), decisions, rowless_projects);
        }
        Union(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions, rowless_projects);
            }
        }
        Minus(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions, rowless_projects);
            }
        }
        Intersect(n) => {
            for child in n.dependencies_mut() {
                changed |= apply_decisions(child, decisions, rowless_projects);
            }
        }
        _ => {}
    }
    changed
}
