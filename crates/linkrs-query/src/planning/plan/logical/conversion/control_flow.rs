//! Physical-to-logical conversion: control flow node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::{convert_plan, ConversionError};

use crate::planning::plan::core::nodes::control_flow::control_flow_node::{
    ArgumentNode, BeginTransactionNode, CommitNode, LoopNode, PassThroughNode, RollbackNode,
    SelectNode,
};

pub(super) fn convert_select(n: &SelectNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let if_branch = if let Some(b) = n.if_branch() {
            Some(Box::new(convert_plan(b)?))
        } else {
            None
        };
        let else_branch = if let Some(b) = n.else_branch() {
            Some(Box::new(convert_plan(b)?))
        } else {
            None
        };
        Ok(LogicalNodeEnum::Select(
            crate::planning::plan::logical::logical_nodes::control_flow::LogicalSelectNode {
                id: n.id(),
                condition: n.condition().clone(),
                if_branch,
                else_branch,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_loop(n: &LoopNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let body = if let Some(b) = n.body() {
            Some(Box::new(convert_plan(b)?))
        } else {
            None
        };
        Ok(LogicalNodeEnum::Loop(
            crate::planning::plan::logical::logical_nodes::control_flow::LogicalLoopNode {
                id: n.id(),
                condition: n.condition().clone(),
                body,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_argument(n: &ArgumentNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::Argument(
        crate::planning::plan::logical::logical_nodes::control_flow::LogicalArgumentNode {
            id: n.id(),
            var: n.var().to_string(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: vec![],
        },
    ))
}

pub(super) fn convert_pass_through(
    n: &PassThroughNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::PassThrough(
        crate::planning::plan::logical::logical_nodes::control_flow::LogicalPassThroughNode {
            id: n.id(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: vec![],
        },
    ))
}

pub(super) fn convert_begin_transaction(
    n: &BeginTransactionNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::BeginTransaction(
        crate::planning::plan::logical::logical_nodes::control_flow::LogicalBeginTransactionNode {
            id: n.id(),
            isolation_level: n.isolation_level(),
            read_only: n.read_only(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: vec![],
        },
    ))
}

pub(super) fn convert_commit(n: &CommitNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::Commit(
        crate::planning::plan::logical::logical_nodes::control_flow::LogicalCommitNode {
            id: n.id(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: vec![],
        },
    ))
}

pub(super) fn convert_rollback(n: &RollbackNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::Rollback(
        crate::planning::plan::logical::logical_nodes::control_flow::LogicalRollbackNode {
            id: n.id(),
            savepoint: n.savepoint().map(|s| s.to_string()),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: vec![],
        },
    ))
}
