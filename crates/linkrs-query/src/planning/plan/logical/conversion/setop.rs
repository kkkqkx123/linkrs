//! Physical-to-logical conversion: setop node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::{convert_plan, ConversionError};

use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::{
    UnionNode, UnwindNode,
};
use crate::planning::plan::core::nodes::graph_operations::set_operations_node::{
    IntersectNode, MinusNode,
};

use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

pub(super) fn convert_union(n: &UnionNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let left = convert_plan(n.input())?;
        let right = convert_plan(n.union_input())?;
        Ok(LogicalNodeEnum::Union(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnionNode {
                id: n.id(),
                deps: vec![left, right],
                distinct: n.distinct(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_minus(n: &MinusNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let left = convert_plan(n.input())?;
        let right = convert_plan(n.minus_input())?;
        Ok(LogicalNodeEnum::Minus(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalMinusNode {
                id: n.id(),
                deps: vec![left, right],
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_intersect(n: &IntersectNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let left = convert_plan(n.input())?;
        let right = convert_plan(n.intersect_input())?;
        Ok(LogicalNodeEnum::Intersect(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalIntersectNode {
                id: n.id(),
                deps: vec![left, right],
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_unwind(n: &UnwindNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Unwind(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                alias: n.alias().to_string(),
                list_expression: n.list_expression().clone(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}
