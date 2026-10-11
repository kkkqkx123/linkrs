//! Physical-to-logical conversion: traversal node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::{convert_plan, ConversionError};

use crate::planning::plan::core::nodes::traversal::traversal_node::{
    AppendVerticesNode, BiExpandNode, BiTraverseNode, ExpandAllNode, ExpandNode, TraverseNode,
};

use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, PlanNode, SingleInputNode,
};

pub(super) fn convert_expand(n: &ExpandNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let deps: Vec<LogicalNodeEnum> = n
            .inputs()
            .iter()
            .map(convert_plan)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LogicalNodeEnum::Expand(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandNode {
                id: n.id(),
                space_id: 1,
                edge_types: n.edge_types().to_vec(),
                direction: n.direction(),
                step_limit: n.step_limit(),
                filter: n.filter().cloned(),
                dst_tag: n.dst_tag().map(|s| s.to_string()),
                deps,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_expand_all(n: &ExpandAllNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let deps: Vec<LogicalNodeEnum> = n
            .inputs()
            .iter()
            .map(convert_plan)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LogicalNodeEnum::ExpandAll(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandAllNode {
                id: n.id(),
                space_id: n.space_id(),
                edge_types: n.edge_types().to_vec(),
                direction: n.direction().to_string(),
                any_edge_type: n.any_edge_type(),
                step_limit: n.step_limit(),
                step_limits: n.step_limits().cloned(),
                join_input: n.join_input(),
                sample: n.sample(),
                edge_props: n.edge_props().to_vec(),
                vertex_props: n.vertex_props().to_vec(),
                filter: n.filter().cloned(),
                src_vids: n.src_vids().to_vec(),
                include_empty_paths: n.include_empty_paths(),
                input_var: n.get_input_var().map(|s| s.to_string()),
                path_semantic: n.path_semantic(),
                dst_tag: n.dst_tag().map(|s| s.to_string()),
                deps,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_traverse(n: &TraverseNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Traverse(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalTraverseNode {
                id: n.id(),
                space_id: 1,
                start_vids: n.start_vids().to_string(),
                end_vids: n.end_vids().cloned(),
                edge_types: n.edge_types().to_vec(),
                direction: n.direction(),
                min_steps: n.min_steps(),
                max_steps: n.max_steps(),
                edge_alias: n.edge_alias().cloned(),
                vertex_alias: n.vertex_alias().cloned(),
                e_filter: n.e_filter().cloned(),
                v_filter: n.v_filter().cloned(),
                first_step_filter: n.first_step_filter().cloned(),
                path_semantic: n.path_semantic(),
                dst_tag: n.dst_tag().map(|s| s.to_string()),
                input: Some(Box::new(logical_input)),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_append_vertices(
    n: &AppendVerticesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let deps: Vec<LogicalNodeEnum> = n
            .inputs()
            .iter()
            .map(convert_plan)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LogicalNodeEnum::AppendVertices(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalAppendVerticesNode {
                id: n.id(),
                space_id: 1,
                vertex_tag: n.vertex_tag().to_string(),
                vertex_props: n.vertex_props().to_vec(),
                filter: n.filter().cloned(),
                input_var: n.input_var().map(|s| s.to_string()),
                src_expression: n.src_expression().cloned(),
                dedup: n.dedup(),
                need_fetch_prop: n.need_fetch_prop(),
                vids: n.vids().to_vec(),
                tag_ids: n.tag_ids().to_vec(),
                v_filter: n.v_filter().cloned(),
                node_alias: n.node_alias().cloned(),
                deps,
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_bi_expand(n: &BiExpandNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_left = convert_plan(n.left_input())?;
        let logical_right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::BiExpand(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalBiExpandNode {
                id: n.id(),
                space_id: 1,
                left_direction: n.left_direction(),
                right_direction: n.right_direction(),
                edge_types: n.edge_types().to_vec(),
                max_hops: n.max_hops(),
                meeting_point_var: n.meeting_point_var().cloned(),
                dst_tag: n.dst_tag().map(|s| s.to_string()),
                left: Box::new(logical_left),
                right: Box::new(logical_right),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}

pub(super) fn convert_bi_traverse(n: &BiTraverseNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_left = convert_plan(n.left_input())?;
        let logical_right = convert_plan(n.right_input())?;
        Ok(LogicalNodeEnum::BiTraverse(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalBiTraverseNode {
                id: n.id(),
                space_id: 1,
                left_src_var: n.left_src_var().to_string(),
                right_src_var: n.right_src_var().to_string(),
                edge_types: n.edge_types().to_vec(),
                left_direction: n.left_direction(),
                right_direction: n.right_direction(),
                min_hops: n.min_hops(),
                max_hops: n.max_hops(),
                path_var: n.path_var().to_string(),
                edge_alias: n.edge_alias().cloned(),
                vertex_alias: n.vertex_alias().cloned(),
                dst_tag: n.dst_tag().map(|s| s.to_string()),
                left: Box::new(logical_left),
                right: Box::new(logical_right),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: vec![],
            },
        ))
    }
}
