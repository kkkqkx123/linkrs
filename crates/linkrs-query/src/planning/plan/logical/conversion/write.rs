//! Physical-to-logical conversion: write node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::{convert_plan, ConversionError};

use crate::planning::plan::core::nodes::access::graph_scan_node::GetEdgesNode;
use crate::planning::plan::core::nodes::data_modification::{
    CopyFromNode, CopyToNode, DeleteEdgesNode, DeleteIndexNode, DeleteVerticesNode,
    InsertEdgesNode, InsertVerticesNode, PipeDeleteEdgesNode, PipeDeleteVerticesNode, UpdateNode,
};

use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

pub(super) fn convert_get_edges(n: &GetEdgesNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::GetEdges(
        crate::planning::plan::logical::logical_nodes::access::LogicalGetEdgesNode {
            id: n.id(),
            space_id: n.space_id(),
            edge_ref: n.edge_ref().clone(),
            src: n.src().to_string(),
            edge_type: n.edge_type().to_string(),
            rank: n.rank().to_string(),
            dst: n.dst().to_string(),
            edge_props: n.edge_props().to_vec(),
            expression: n.filter().cloned(),
            dedup: n.dedup(),
            limit: n.limit(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_insert_vertices(
    n: &InsertVerticesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::InsertVertices(
        crate::planning::plan::logical::logical_nodes::dml::LogicalInsertVerticesNode {
            id: n.id(),
            info: n.info().clone(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_insert_edges(
    n: &InsertEdgesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::InsertEdges(
        crate::planning::plan::logical::logical_nodes::dml::LogicalInsertEdgesNode {
            id: n.id(),
            info: n.info().clone(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_update(n: &UpdateNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::Update(
        crate::planning::plan::logical::logical_nodes::dml::LogicalUpdateNode {
            id: n.id(),
            info: n.info().clone(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_delete_vertices(
    n: &DeleteVerticesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::DeleteVertices(
        crate::planning::plan::logical::logical_nodes::dml::LogicalDeleteVerticesNode {
            id: n.id(),
            info: n.info().clone(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_delete_edges(
    n: &DeleteEdgesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::DeleteEdges(
        crate::planning::plan::logical::logical_nodes::dml::LogicalDeleteEdgesNode {
            id: n.id(),
            info: n.info().clone(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_delete_index(
    n: &DeleteIndexNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::DeleteIndex(
        crate::planning::plan::logical::logical_nodes::dml::LogicalDeleteIndexNode {
            id: n.id(),
            info: n.info().clone(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_pipe_delete_vertices(
    n: &PipeDeleteVerticesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::PipeDeleteVertices(
            crate::planning::plan::logical::logical_nodes::dml::LogicalPipeDeleteVerticesNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                info: n.info().clone(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_pipe_delete_edges(
    n: &PipeDeleteEdgesNode,
) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::PipeDeleteEdges(
            crate::planning::plan::logical::logical_nodes::dml::LogicalPipeDeleteEdgesNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                info: n.info().clone(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_copy_from(n: &CopyFromNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::CopyFrom(
        crate::planning::plan::logical::logical_nodes::dml::LogicalCopyFromNode {
            id: n.id(),
            space_name: n.space_name().to_string(),
            target: n.target().clone(),
            file_paths: n.file_paths().to_vec(),
            by_column: n.by_column(),
            header: n.header(),
            delimiter: n.delimiter(),
            batch_size: n.batch_size(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}

pub(super) fn convert_copy_to(n: &CopyToNode) -> Result<LogicalNodeEnum, ConversionError> {
    Ok(LogicalNodeEnum::CopyTo(
        crate::planning::plan::logical::logical_nodes::dml::LogicalCopyToNode {
            id: n.id(),
            space_name: n.space_name().to_string(),
            target: n.target().clone(),
            file_path: n.file_path().to_string(),
            header: n.header(),
            delimiter: n.delimiter(),
            output_var: n.output_var().map(|s| s.to_string()),
            col_names: n.col_names().to_vec(),
            column_types: n.column_types().to_vec(),
        },
    ))
}
