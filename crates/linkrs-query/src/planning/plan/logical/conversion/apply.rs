//! Physical-to-logical conversion: apply node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::{convert_plan, ConversionError};

use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::{
    ApplyNode, CorrelatedApplyNode, PatternApplyNode, RollUpApplyNode,
};
use crate::planning::plan::core::nodes::join::wco_intersect_node::WcoIntersectNode;

use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

pub(super) fn convert_apply(n: &ApplyNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let left = convert_plan(n.left_input())?;
        let right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::Apply(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalApplyNode {
                id: n.id(),
                left: Box::new(left),
                right: Box::new(right),
                left_input_var: None,
                right_input_var: None,
                correlated_cols: n.correlated_cols().to_vec(),
                apply_kind: n.apply_kind(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_pattern_apply(
    n: &PatternApplyNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let left = convert_plan(n.left_input())?;
        let right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::PatternApply(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalPatternApplyNode {
                id: n.id(),
                left: Box::new(left),
                right: Box::new(right),
                hash_keys: n.hash_keys().to_vec(),
                probe_keys: n.probe_keys().to_vec(),
                is_anti_predicate: n.is_anti_predicate(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_roll_up_apply(
    n: &RollUpApplyNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::RollUpApply(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalRollUpApplyNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                left_input_var: n.left_input_var().map(|s| s.to_string()),
                right_input_var: n.right_input_var().map(|s| s.to_string()),
                compare_cols: n.compare_cols().to_vec(),
                collect_col: n.collect_col().cloned(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_correlated_apply(
    n: &CorrelatedApplyNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let left = convert_plan(n.left_input())?;
        let right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::CorrelatedApply(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalCorrelatedApplyNode {
                id: n.id(),
                left: Box::new(left),
                right: Box::new(right),
                hash_keys: vec![],
                probe_keys: vec![],
                is_anti_predicate: n.is_anti_predicate(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }

    // Physical join nodes → logical equivalents
}

pub(super) fn convert_wco_intersect(
    n: &WcoIntersectNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let probe = convert_plan(n.probe_input())?;
        let mut builds = Vec::with_capacity(n.num_builds());
        for build_input in n.build_inputs() {
            builds.push(convert_plan(build_input)?);
        }
        let mut logical =
                crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode::new(
                    probe,
                    builds,
                    n.intersect_key().clone(),
                    n.bound_keys().to_vec(),
                    n.col_names().to_vec(),
                );
        if let Some(var) = n.output_var() {
            logical.set_output_var(var.to_string());
        }
        logical.set_column_types(n.column_types().to_vec());
        Ok(LogicalNodeEnum::WcoIntersect(logical))
    }
}
