use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::operators::spec::{
    SinkSpec
};

use linkrs_core::types::expr::Expression;

use super::super::contextual_to_expression;

// ── Sink spec builders ────────────────────────────────────────────────────────

pub(in crate::executor::streaming::plan::arena_builder) fn build_insert_vertices_spec(
    node: &crate::planning::plan::core::nodes::data_modification::insert_nodes::InsertVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let tag_property_names: Vec<String> = node.prop_names().to_vec();
    Ok(SinkSpec::InsertVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        vertex_properties: std::iter::once((
            "vid".to_string(),
            Expression::Variable("vid".to_string()),
        ))
        .chain(
            tag_property_names
                .iter()
                .map(|name| (name.clone(), Expression::Variable(name.clone()))),
        )
        .collect(),
        tag: node.tag_name().to_string(),
        tag_property_names,
        if_not_exists: node.info().if_not_exists,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_insert_edges_spec(
    node: &crate::planning::plan::core::nodes::data_modification::insert_nodes::InsertEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::InsertEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        rank_col: "rank".to_string(),
        edge_type: node.edge_name().to_string(),
        edge_properties: node
            .prop_names()
            .iter()
            .map(|name| (name.clone(), Expression::Variable(name.clone())))
            .collect(),
        if_not_exists: node.info().if_not_exists,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_delete_vertices_spec(
    node: &crate::planning::plan::core::nodes::data_modification::delete_nodes::DeleteVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let tag = node.tag().ok_or_else(|| {
        PlanBuildError::missing_value(
            "DeleteVertices",
            node.id(),
            "tag",
            "DELETE VERTEX requires a tag qualifier",
        )
    })?;
    Ok(SinkSpec::DeleteVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        tag: tag.to_string(),
        vertex_id_col: "vid".to_string(),
        cascade: node.cascade(),
    })
}

fn pipe_reference_column(expr: &linkrs_core::types::expr::ContextualExpression) -> Option<String> {
    use linkrs_core::types::expr::Expression;
    let inner = expr.expression()?;
    match inner.inner() {
        Expression::Variable(name) if name != "$-" => Some(name.clone()),
        Expression::Property { object, property } => {
            if let Expression::Variable(base) = object.as_ref() {
                if base == "$-" {
                    return Some(property.clone());
                }
            }
            Some(property.clone())
        }
        _ => None,
    }
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_delete_edges_spec(
    node: &crate::planning::plan::core::nodes::data_modification::delete_nodes::DeleteEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let edge_type = node.edge_type().unwrap_or("").to_string();
    Ok(SinkSpec::DeleteEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        rank_col: "rank".to_string(),
        edge_type,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_pipe_delete_vertices_spec(
    node: &crate::planning::plan::core::nodes::data_modification::delete_nodes::PipeDeleteVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let first = node.vertex_ids().first();
    let vertex_id_col = first
        .and_then(pipe_reference_column)
        .unwrap_or_else(|| "vid".to_string());
    let vertex_id_is_literal = first.and_then(pipe_reference_column).is_none();
    Ok(SinkSpec::PipeDeleteVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        vertex_id_col,
        cascade: node.cascade(),
        vertex_id_is_literal,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_pipe_delete_edges_spec(
    node: &crate::planning::plan::core::nodes::data_modification::delete_nodes::PipeDeleteEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let edge_type = node.edge_type().unwrap_or("").to_string();
    let (src_col, dst_col) = node
        .edges()
        .first()
        .map(|(src, dst, _)| {
            (
                pipe_reference_column(src).unwrap_or_else(|| "src".to_string()),
                pipe_reference_column(dst).unwrap_or_else(|| "dst".to_string()),
            )
        })
        .unwrap_or_else(|| ("src".into(), "dst".to_string()));
    Ok(SinkSpec::PipeDeleteEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col,
        dst_col,
        rank_col: "rank".to_string(),
        edge_type,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_update_spec(
    node: &crate::planning::plan::core::nodes::data_modification::update_nodes::UpdateNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    use crate::planning::plan::core::nodes::data_modification::info::UpdateTargetType;
    match node.info() {
        UpdateTargetType::Vertex(info) => Ok(SinkSpec::UpdateVertices {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            tag_name: info.tag_name.clone().unwrap_or_default(),
            updates: info
                .properties
                .iter()
                .filter_map(|(name, value)| value.get_expression().map(|expr| (name.clone(), expr)))
                .collect(),
            condition: info
                .condition
                .as_ref()
                .map(contextual_to_expression)
                .transpose()?,
            is_upsert: info.is_upsert,
            replace_properties: info.replace_properties,
        }),
        UpdateTargetType::Edge(info) => Ok(SinkSpec::UpdateEdges {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            src_col: "src".to_string(),
            dst_col: "dst".to_string(),
            rank_col: "rank".to_string(),
            edge_type: info.edge_type.clone().unwrap_or_default(),
            updates: info
                .properties
                .iter()
                .filter_map(|(name, value)| value.get_expression().map(|expr| (name.clone(), expr)))
                .collect(),
            condition: info
                .condition
                .as_ref()
                .map(contextual_to_expression)
                .transpose()?,
            is_upsert: info.is_upsert,
            replace_properties: info.replace_properties,
        }),
    }
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_copy_from_spec(
    node: &crate::planning::plan::core::nodes::data_modification::copy_nodes::CopyFromNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let target = match node.target() {
        crate::planning::plan::core::nodes::data_modification::copy_nodes::CopyTarget::Vertex(
            tag,
        ) => crate::executor::streaming::operators::spec::CopyTarget::Vertex(tag.clone()),
        crate::planning::plan::core::nodes::data_modification::copy_nodes::CopyTarget::Edge(
            edge,
        ) => crate::executor::streaming::operators::spec::CopyTarget::Edge(edge.clone()),
    };
    Ok(SinkSpec::CopyFrom {
        space_name: exec_ctx
            .space_name
            .clone()
            .unwrap_or_else(|| node.space_name().to_string()),
        target,
        file_paths: node.file_paths().to_vec(),
        by_column: node.by_column(),
        header: node.header(),
        delimiter: node.delimiter() as u8,
        batch_size: node.batch_size(),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_copy_to_spec(
    node: &crate::planning::plan::core::nodes::data_modification::copy_nodes::CopyToNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let target = match node.target() {
        crate::planning::plan::core::nodes::data_modification::copy_nodes::CopyTarget::Vertex(
            tag,
        ) => crate::executor::streaming::operators::spec::CopyTarget::Vertex(tag.clone()),
        crate::planning::plan::core::nodes::data_modification::copy_nodes::CopyTarget::Edge(
            edge,
        ) => crate::executor::streaming::operators::spec::CopyTarget::Edge(edge.clone()),
    };
    Ok(SinkSpec::CopyTo {
        space_name: exec_ctx
            .space_name
            .clone()
            .unwrap_or_else(|| node.space_name().to_string()),
        target,
        file_path: node.file_path().to_string(),
        header: node.header(),
        delimiter: node.delimiter() as u8,
    })
}

// ── DDL spec builders ─────────────────────────────────────────────────────────
