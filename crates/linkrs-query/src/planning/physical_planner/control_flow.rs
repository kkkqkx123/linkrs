use super::*;

pub(super) fn convert_argument(
    n: crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode::new(
            next_node_id(),
            &n.var,
        );
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::Argument(node)
}

pub(super) fn convert_loop(
    n: crate::planning::plan::logical::logical_nodes::control_flow::LogicalLoopNode,
) -> PlanNodeEnum {
    let body = n.body().cloned().map(convert_logical_to_physical);
    let mut node =
        crate::planning::plan::core::nodes::control_flow::control_flow_node::LoopNode::new(
            next_node_id(),
            n.condition().clone(),
        );
    if let Some(b) = body {
        node.set_body(b);
    }
    if let Some(var) = n.output_var() {
        node.set_output_var(var.to_string());
    }
    node.set_col_names(n.col_names().to_vec());
    PlanNodeEnum::Loop(node)
}

pub(super) fn convert_pass_through(
    n: crate::planning::plan::logical::logical_nodes::control_flow::LogicalPassThroughNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::control_flow::control_flow_node::PassThroughNode::new(
            next_node_id(),
        );
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    node.set_column_types(n.column_types);
    PlanNodeEnum::PassThrough(node)
}

pub(super) fn convert_select(
    n: crate::planning::plan::logical::logical_nodes::control_flow::LogicalSelectNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::control_flow::control_flow_node::SelectNode::new(
            next_node_id(),
            n.condition().clone(),
        );
    if let Some(if_branch) = n.if_branch().cloned() {
        node.set_if_branch(convert_logical_to_physical(if_branch));
    }
    if let Some(else_branch) = n.else_branch().cloned() {
        node.set_else_branch(convert_logical_to_physical(else_branch));
    }
    if let Some(var) = n.output_var() {
        node.set_output_var(var.to_string());
    }
    node.set_col_names(n.col_names().to_vec());
    PlanNodeEnum::Select(node)
}

pub(super) fn convert_begin_transaction(
    n: crate::planning::plan::logical::logical_nodes::control_flow::LogicalBeginTransactionNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::control_flow::control_flow_node::BeginTransactionNode::new(next_node_id());
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::BeginTransaction(node)
}

pub(super) fn convert_commit(
    n: crate::planning::plan::logical::logical_nodes::control_flow::LogicalCommitNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::control_flow::control_flow_node::CommitNode::new(
            next_node_id(),
        );
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::Commit(node)
}

pub(super) fn convert_rollback(
    n: crate::planning::plan::logical::logical_nodes::control_flow::LogicalRollbackNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::control_flow::control_flow_node::RollbackNode::new(
            next_node_id(),
        );
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    node.set_col_names(n.col_names);
    PlanNodeEnum::Rollback(node)
}
