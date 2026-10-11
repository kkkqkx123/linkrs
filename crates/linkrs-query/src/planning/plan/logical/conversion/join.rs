//! Physical-to-logical conversion: join node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::{convert_plan, ConversionError};

use crate::planning::plan::core::nodes::join::join_node::{
    CrossJoinNode, FullOuterJoinNode, InnerJoinNode, LeftJoinNode, RightJoinNode, SemiJoinNode,
};
use crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode;

use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

pub(super) fn convert_inner_join(n: &InnerJoinNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_left = convert_plan(n.left_input())?;
        let logical_right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::InnerJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode {
                id: n.id(),
                left: Box::new(logical_left),
                right: Box::new(logical_right),
                hash_keys: n.hash_keys().to_vec(),
                probe_keys: n.probe_keys().to_vec(),
                recommended_algorithm: None,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_left_join(n: &LeftJoinNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_left = convert_plan(n.left_input())?;
        let logical_right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::LeftJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode {
                id: n.id(),
                left: Box::new(logical_left),
                right: Box::new(logical_right),
                hash_keys: n.hash_keys().to_vec(),
                probe_keys: n.probe_keys().to_vec(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_right_join(n: &RightJoinNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_left = convert_plan(n.left_input())?;
        let logical_right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::RightJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalRightJoinNode {
                id: n.id(),
                left: Box::new(logical_left),
                right: Box::new(logical_right),
                hash_keys: n.hash_keys().to_vec(),
                probe_keys: n.probe_keys().to_vec(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_cross_join(n: &CrossJoinNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_left = convert_plan(n.left_input())?;
        let logical_right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::CrossJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalCrossJoinNode {
                id: n.id(),
                left: Box::new(logical_left),
                right: Box::new(logical_right),
                hash_keys: vec![],
                probe_keys: vec![],
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_full_outer_join(
    n: &FullOuterJoinNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_left = convert_plan(n.left_input())?;
        let logical_right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::FullOuterJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalFullOuterJoinNode {
                id: n.id(),
                left: Box::new(logical_left),
                right: Box::new(logical_right),
                hash_keys: n.hash_keys().to_vec(),
                probe_keys: n.probe_keys().to_vec(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_semi_join(n: &SemiJoinNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_left = convert_plan(n.left_input())?;
        let logical_right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::SemiJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalSemiJoinNode {
                id: n.id(),
                left: Box::new(logical_left),
                right: Box::new(logical_right),
                hash_keys: n.hash_keys().to_vec(),
                probe_keys: n.probe_keys().to_vec(),
                join_condition: n.join_condition().cloned(),
                anti: n.is_anti(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_flatten(n: &FlattenNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Flatten(
            crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode {
                id: n.id(),
                group_pos: n.group_pos(),
                group_columns: n.group_columns().to_vec(),
                expected_groups: n.expected_groups(),
                input: Some(Box::new(logical_input)),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}
