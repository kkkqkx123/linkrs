use super::*;

pub(super) fn convert_insert_vertices(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalInsertVerticesNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::data_modification::InsertVerticesNode::new(
        n.id, n.info,
    );
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::InsertVertices(node)
}

pub(super) fn convert_insert_edges(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalInsertEdgesNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::data_modification::InsertEdgesNode::new(n.id, n.info);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::InsertEdges(node)
}

pub(super) fn convert_update(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalUpdateNode,
) -> PlanNodeEnum {
    let mut node =
        crate::planning::plan::core::nodes::data_modification::UpdateNode::new(n.id, n.info);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::Update(node)
}

pub(super) fn convert_delete_vertices(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalDeleteVerticesNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::DeleteVerticesNode::new(n.id, n.info);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::DeleteVertices(node)
}

pub(super) fn convert_delete_edges(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalDeleteEdgesNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::DeleteEdgesNode::new(n.id, n.info);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::DeleteEdges(node)
}

pub(super) fn convert_delete_index(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalDeleteIndexNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::DeleteIndexNode::new(n.id, n.info);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::DeleteIndex(node)
}

pub(super) fn convert_pipe_delete_vertices(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalPipeDeleteVerticesNode,
) -> PlanNodeEnum {
    let input = n.input.expect("PipeDeleteVertices requires an input node");
    let physical_input = convert_logical_to_physical(*input);
    let mut node = crate::planning::plan::core::nodes::PipeDeleteVerticesNode::new(
        n.id,
        n.info,
        physical_input,
    );
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::PipeDeleteVertices(node)
}

pub(super) fn convert_pipe_delete_edges(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalPipeDeleteEdgesNode,
) -> PlanNodeEnum {
    let input = n.input.expect("PipeDeleteEdges requires an input node");
    let physical_input = convert_logical_to_physical(*input);
    let mut node =
        crate::planning::plan::core::nodes::PipeDeleteEdgesNode::new(n.id, n.info, physical_input);
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::PipeDeleteEdges(node)
}

pub(super) fn convert_copy_from(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalCopyFromNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::CopyFromNode::new(
        n.id,
        n.space_name.clone(),
        n.target.clone(),
        n.file_paths.clone(),
        n.by_column,
        n.header,
        n.delimiter,
        n.batch_size,
    );
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::CopyFrom(node)
}

pub(super) fn convert_copy_to(
    n: crate::planning::plan::logical::logical_nodes::dml::LogicalCopyToNode,
) -> PlanNodeEnum {
    let mut node = crate::planning::plan::core::nodes::CopyToNode::new(
        n.id,
        n.space_name.clone(),
        n.target.clone(),
        n.file_path.clone(),
        n.header,
        n.delimiter,
    );
    if let Some(var) = n.output_var {
        node.set_output_var(var);
    }
    if !n.col_names.is_empty() {
        node.set_col_names(n.col_names);
    }
    if !n.column_types.is_empty() {
        node.set_column_types(n.column_types);
    }
    PlanNodeEnum::CopyTo(node)
}
