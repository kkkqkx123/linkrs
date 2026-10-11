//! Physical-to-logical conversion: pipeline node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::{convert_plan, ConversionError};

use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::{
    AssignNode, DataCollectNode, MaterializeNode, RemoveNode,
};

use crate::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, SingleInputNode};

pub(super) fn convert_assign(n: &AssignNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Assign(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                assignments: n.assignments().to_vec(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_remove(n: &RemoveNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Remove(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalRemoveNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                remove_items: n.remove_items().to_vec(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_data_collect(
    n: &DataCollectNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::DataCollect(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalDataCollectNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                collect_kind: n.collect_kind().to_string(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_materialize(n: &MaterializeNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Materialize(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalMaterializeNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}
