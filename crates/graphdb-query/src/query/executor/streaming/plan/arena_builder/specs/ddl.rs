//! Sink (write), DDL manage, fulltext and vector spec builders.

use crate::core::types::expr::Expression;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::{
    DdlSpec, EdgeManageCommand, FulltextManageCommand, FulltextSpec, IndexManageCommand,
    PropertyRename, SinkSpec, SpaceManageCommand, TagManageCommand, UserManageCommand,
    VectorManageCommand, VectorSpec,
};

use super::contextual_to_expression;

// ── Sink spec builders ────────────────────────────────────────────────────────

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_insert_vertices_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::insert_nodes::InsertVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let tag_property_names: Vec<Vec<String>> = node
        .tags()
        .iter()
        .map(|tag| tag.prop_names.clone())
        .collect();
    Ok(SinkSpec::InsertVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        vertex_properties: std::iter::once((
            "vid".to_string(),
            Expression::Variable("vid".to_string()),
        ))
        .chain(
            tag_property_names
                .iter()
                .flatten()
                .map(|name| (name.clone(), Expression::Variable(name.clone()))),
        )
        .collect(),
        tags: node.tag_names(),
        tag_property_names,
        if_not_exists: node.info().if_not_exists,
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_insert_edges_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::insert_nodes::InsertEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::InsertEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        edge_type: node.edge_name().to_string(),
        edge_properties: node
            .prop_names()
            .iter()
            .map(|name| (name.clone(), Expression::Variable(name.clone())))
            .collect(),
        if_not_exists: node.info().if_not_exists,
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_delete_vertices_spec(
    _node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::DeleteVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::DeleteVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        vertex_id_col: "vid".to_string(),
    })
}

fn pipe_reference_column(expr: &crate::core::types::expr::ContextualExpression) -> Option<String> {
    use crate::core::types::expr::Expression;
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

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_delete_edges_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::DeleteEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let edge_type = node.edge_type().unwrap_or("").to_string();
    Ok(SinkSpec::DeleteEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        edge_type,
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_delete_tags_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::DeleteTagsNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::DeleteTags {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        tag_names: node.tag_names().to_vec(),
        vertex_ids: None,
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_pipe_delete_vertices_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::PipeDeleteVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let vertex_id_col = node
        .vertex_ids()
        .first()
        .and_then(pipe_reference_column)
        .unwrap_or_else(|| "vid".to_string());
    Ok(SinkSpec::PipeDeleteVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        vertex_id_col,
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_pipe_delete_edges_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::PipeDeleteEdgesNode,
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
        .unwrap_or_else(|| ("src".to_string(), "dst".to_string()));
    Ok(SinkSpec::PipeDeleteEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col,
        dst_col,
        edge_type,
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_update_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::update_nodes::UpdateNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    use crate::query::planning::plan::core::nodes::data_modification::info::UpdateTargetType;
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
        }),
        UpdateTargetType::Edge(info) => Ok(SinkSpec::UpdateEdges {
            space_name: exec_ctx.space_name.clone().unwrap_or_default(),
            src_col: "src".to_string(),
            dst_col: "dst".to_string(),
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
        }),
    }
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_update_vertices_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::update_nodes::UpdateVerticesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let tag_name = node
        .updates()
        .first()
        .and_then(|update| update.tag_name.clone())
        .unwrap_or_default();
    Ok(SinkSpec::UpdateVertices {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        tag_name,
        updates: node
            .updates()
            .iter()
            .flat_map(|update| update.properties.iter())
            .map(|(name, value)| contextual_to_expression(value).map(|expr| (name.clone(), expr)))
            .collect::<Result<Vec<_>, _>>()?,
        condition: node
            .updates()
            .first()
            .and_then(|update| update.condition.as_ref())
            .map(contextual_to_expression)
            .transpose()?,
        is_upsert: node
            .updates()
            .first()
            .map(|update| update.is_upsert)
            .unwrap_or(false),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_update_edges_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::update_nodes::UpdateEdgesNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    Ok(SinkSpec::UpdateEdges {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        src_col: "src".to_string(),
        dst_col: "dst".to_string(),
        edge_type: node
            .updates()
            .first()
            .and_then(|update| update.edge_type.clone())
            .unwrap_or_default(),
        updates: node
            .updates()
            .iter()
            .flat_map(|update| update.properties.iter())
            .map(|(name, value)| contextual_to_expression(value).map(|expr| (name.clone(), expr)))
            .collect::<Result<Vec<_>, _>>()?,
        condition: node
            .updates()
            .first()
            .and_then(|update| update.condition.as_ref())
            .map(contextual_to_expression)
            .transpose()?,
        is_upsert: node
            .updates()
            .first()
            .map(|update| update.is_upsert)
            .unwrap_or(false),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_copy_from_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::copy_nodes::CopyFromNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let target = match node.target() {
        crate::query::planning::plan::core::nodes::data_modification::copy_nodes::CopyTarget::Vertex(tag) => {
            crate::query::executor::streaming::operators::spec::CopyTarget::Vertex(tag.clone())
        }
        crate::query::planning::plan::core::nodes::data_modification::copy_nodes::CopyTarget::Edge(edge) => {
            crate::query::executor::streaming::operators::spec::CopyTarget::Edge(edge.clone())
        }
    };
    Ok(SinkSpec::CopyFrom {
        space_name: exec_ctx
            .space_name
            .clone()
            .unwrap_or_else(|| node.space_name().to_string()),
        target,
        file_path: node.file_path().to_string(),
        header: node.header(),
        delimiter: node.delimiter() as u8,
        batch_size: node.batch_size(),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_copy_to_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::copy_nodes::CopyToNode,
    exec_ctx: &ExecutionContext,
) -> Result<SinkSpec, PlanBuildError> {
    let target = match node.target() {
        crate::query::planning::plan::core::nodes::data_modification::copy_nodes::CopyTarget::Vertex(tag) => {
            crate::query::executor::streaming::operators::spec::CopyTarget::Vertex(tag.clone())
        }
        crate::query::planning::plan::core::nodes::data_modification::copy_nodes::CopyTarget::Edge(edge) => {
            crate::query::executor::streaming::operators::spec::CopyTarget::Edge(edge.clone())
        }
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

fn space_manage_to_command(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode,
) -> SpaceManageCommand {
    use crate::query::executor::streaming::operators::spec::SpaceManageCommand;
    use crate::query::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode::*;
    match node {
        Create(n) => SpaceManageCommand::Create {
            space_name: n.info().space_name.clone(),
            vid_type: n.info().vid_type.clone(),
        },
        Drop(n) => SpaceManageCommand::Drop {
            space_name: n.space_name().to_string(),
        },
        Desc(n) => SpaceManageCommand::Desc {
            space_name: n.space_name().to_string(),
        },
        Show(_) => SpaceManageCommand::Show,
        ShowCreate(n) => SpaceManageCommand::ShowCreate {
            space_name: n.space_name().to_string(),
        },
        Switch(n) => SpaceManageCommand::Switch {
            space_name: n.space_name().to_string(),
        },
        Alter(n) => SpaceManageCommand::Alter {
            space_name: n.space_name().to_string(),
        },
        Clear(n) => SpaceManageCommand::Clear {
            space_name: n.space_name().to_string(),
        },
    }
}

fn tag_manage_to_command(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::TagManageNode,
) -> TagManageCommand {
    use crate::query::executor::streaming::operators::spec::TagManageCommand;
    use crate::query::planning::plan::core::nodes::management::manage_node_enums::TagManageNode::*;
    match node {
        Create(n) => {
            let info = n.info();
            TagManageCommand::Create {
                tag_name: info.tag_name.clone(),
                properties: info.properties.clone(),
                if_not_exists: info.if_not_exists,
            }
        }
        Alter(n) => {
            let info = n.info();
            TagManageCommand::Alter {
                tag_name: info.tag_name.clone(),
                additions: info.additions.clone(),
                deletions: info.deletions.clone(),
                changes: info
                    .changes
                    .iter()
                    .map(|c| PropertyRename {
                        old_name: c.old_name.clone(),
                        new_name: c.new_name.clone(),
                    })
                    .collect(),
            }
        }
        Desc(n) => TagManageCommand::Desc {
            tag_name: n.tag_name().to_string(),
        },
        Drop(n) => TagManageCommand::Drop {
            tag_name: n.tag_name().to_string(),
            if_exists: n.if_exists(),
        },
        Show(_) => TagManageCommand::Show,
        ShowCreate(n) => TagManageCommand::ShowCreate {
            tag_name: n.tag_name().to_string(),
        },
    }
}

fn edge_manage_to_command(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode,
) -> EdgeManageCommand {
    use crate::query::executor::streaming::operators::spec::EdgeManageCommand;
    use crate::query::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode::*;
    match node {
        Create(n) => {
            let info = n.info();
            EdgeManageCommand::Create {
                edge_name: info.edge_name.clone(),
                properties: info.properties.clone(),
                src_tag_name: info.src_tag_name.clone(),
                dst_tag_name: info.dst_tag_name.clone(),
                if_not_exists: info.if_not_exists,
            }
        }
        Alter(n) => {
            let info = n.info();
            EdgeManageCommand::Alter {
                edge_name: info.edge_name.clone(),
                additions: info.additions.clone(),
                deletions: info.deletions.clone(),
            }
        }
        Desc(n) => EdgeManageCommand::Desc {
            edge_name: n.edge_name().to_string(),
        },
        Drop(n) => EdgeManageCommand::Drop {
            edge_name: n.edge_name().to_string(),
            if_exists: n.if_exists(),
        },
        Show(_) => EdgeManageCommand::Show,
        ShowCreate(n) => EdgeManageCommand::ShowCreate {
            edge_name: n.edge_name().to_string(),
        },
    }
}

fn index_manage_to_command(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode,
) -> IndexManageCommand {
    use crate::query::executor::streaming::operators::spec::IndexManageCommand;
    use crate::query::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode::*;
    match node {
        CreateTagIndex(n) => {
            let info = n.info();
            IndexManageCommand::CreateTagIndex {
                index_name: info.index_name.clone(),
                target_name: info.target_name.clone(),
                properties: info.properties.clone(),
            }
        }
        DropTagIndex(n) => IndexManageCommand::DropTagIndex {
            index_name: n.index_name().to_string(),
        },
        DescTagIndex(n) => IndexManageCommand::DescTagIndex {
            index_name: n.index_name().to_string(),
        },
        ShowTagIndexes(_) => IndexManageCommand::ShowTagIndexes,
        RebuildTagIndex(n) => IndexManageCommand::RebuildTagIndex {
            index_name: n.index_name().to_string(),
        },
        CreateEdgeIndex(n) => {
            let info = n.info();
            IndexManageCommand::CreateEdgeIndex {
                index_name: info.index_name.clone(),
                target_name: info.target_name.clone(),
                properties: info.properties.clone(),
            }
        }
        DropEdgeIndex(n) => IndexManageCommand::DropEdgeIndex {
            index_name: n.index_name().to_string(),
        },
        DescEdgeIndex(n) => IndexManageCommand::DescEdgeIndex {
            index_name: n.index_name().to_string(),
        },
        ShowEdgeIndexes(_) => IndexManageCommand::ShowEdgeIndexes,
        RebuildEdgeIndex(n) => IndexManageCommand::RebuildEdgeIndex {
            index_name: n.index_name().to_string(),
        },
        ShowIndexes(_) => IndexManageCommand::ShowIndexes,
        ShowCreateIndex(n) => IndexManageCommand::ShowCreateIndex {
            index_name: n.index_name().to_string(),
        },
    }
}

fn user_manage_to_command(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode,
) -> UserManageCommand {
    use crate::query::executor::streaming::operators::spec::UserManageCommand;
    use crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode::*;
    match node {
        Create(n) => UserManageCommand::Create {
            username: n.username().to_string(),
            password: n.password().to_string(),
            role: n.role().to_string(),
        },
        Alter(n) => UserManageCommand::Alter {
            username: n.username().to_string(),
            new_password: n.new_password().cloned(),
            new_role: n.new_role().cloned(),
            is_locked: n.is_locked(),
        },
        Drop(n) => UserManageCommand::Drop {
            username: n.username().to_string(),
            if_exists: n.if_exists(),
        },
        ChangePassword(n) => UserManageCommand::ChangePassword {
            password_info: n.password_info().clone(),
        },
        GrantRole(n) => UserManageCommand::GrantRole {
            username: n.username().to_string(),
            space_name: n.space_name().to_string(),
            role: n.role().to_string(),
        },
        RevokeRole(n) => UserManageCommand::RevokeRole {
            username: n.username().to_string(),
            space_name: n.space_name().to_string(),
        },
        ShowUsers(_) => UserManageCommand::ShowUsers,
        ShowRoles(_) => UserManageCommand::ShowRoles,
        DescribeUser(n) => UserManageCommand::DescribeUser {
            username: n.username().to_string(),
        },
    }
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_space_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::SpaceManage {
        command: space_manage_to_command(node),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_tag_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::TagManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::TagManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: tag_manage_to_command(node),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_edge_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::EdgeManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: edge_manage_to_command(node),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_index_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::IndexManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: index_manage_to_command(node),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_delete_index_spec(
    node: &crate::query::planning::plan::core::nodes::data_modification::delete_nodes::DeleteIndexNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::DeleteIndex {
        space_name: node.info().space_name.clone(),
        index_name: node.info().index_name.clone(),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_user_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::UserManage {
        command: user_manage_to_command(node),
    })
}

// ── Fulltext spec builders ────────────────────────────────────────────────────

fn fulltext_manage_to_command(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode,
) -> FulltextManageCommand {
    use crate::query::executor::streaming::operators::spec::FulltextManageCommand;
    use crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode::*;
    match node {
        Create(n) => FulltextManageCommand::Create {
            index_name: n.index_name.clone(),
            schema_name: n.schema_name.clone(),
            fields: n.fields.iter().map(|f| f.field_name.clone()).collect(),
            space_id: n.space_id,
        },
        Drop(n) => FulltextManageCommand::Drop {
            index_name: n.index_name.clone(),
            if_exists: n.if_exists,
        },
        Alter(n) => FulltextManageCommand::Alter {
            index_name: n.index_name.clone(),
        },
        Show(n) => FulltextManageCommand::Show {
            pattern: n.pattern.clone(),
            from_schema: n.from_schema.clone(),
        },
        Describe(n) => FulltextManageCommand::Describe {
            index_name: n.index_name.clone(),
        },
    }
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_fulltext_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::FulltextManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: fulltext_manage_to_command(node),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_fulltext_search_spec(
    node: &crate::query::planning::plan::core::nodes::search::fulltext::data_access::FulltextSearchNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::FulltextSearch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: exec_ctx.current_space_id().unwrap_or(0),
        index_name: node.index_name.clone(),
        search_query: fulltext_query_to_string(&node.query),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_fulltext_lookup_spec(
    node: &crate::query::planning::plan::core::nodes::search::fulltext::data_access::FulltextLookupNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::FulltextLookup {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: exec_ctx.current_space_id().unwrap_or(0),
        index_name: node.index_name.clone(),
        search_query: node.query.clone(),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
    })
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_match_fulltext_spec(
    node: &crate::query::planning::plan::core::nodes::search::fulltext::data_access::MatchFulltextNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::MatchFulltext {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        match_expr: Expression::Literal(crate::core::Value::string(format!(
            "{}:{}",
            node.fulltext_condition.field, node.fulltext_condition.query
        ))),
        match_field: Some(node.field_name.clone()),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
    })
}

// ── Vector spec builders ──────────────────────────────────────────────────────

fn vector_manage_to_command(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode,
) -> VectorManageCommand {
    use crate::query::executor::streaming::operators::spec::VectorManageCommand;
    use crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode::*;
    match node {
        Create(n) => VectorManageCommand::Create {
            index_name: n.index_name.clone(),
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
            vector_size: n.vector_size,
            distance: n.distance,
            space_id: n.space_id,
        },
        Drop(n) => VectorManageCommand::Drop {
            index_name: n.index_name.clone(),
            if_exists: n.if_exists,
            space_id: n.space_id,
            tag_name: n.tag_name.clone(),
            field_name: n.field_name.clone(),
        },
    }
}

pub(in crate::query::executor::streaming::plan::arena_builder) fn build_vector_manage_spec(
    node: &crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    Ok(VectorSpec::VectorManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: vector_manage_to_command(node),
    })
}

#[cfg(feature = "vector")]
pub(in crate::query::executor::streaming::plan::arena_builder) fn build_vector_search_spec(
    node: &crate::query::planning::plan::core::nodes::search::vector::data_access::VectorSearchNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    let query_vector = vector_query_to_vec(&node.query)?;
    Ok(VectorSpec::VectorSearch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: node.space_id,
        index_name: node.index_name.clone(),
        query_vector,
        top_k: node.limit as u32,
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        threshold: node.threshold,
        filter: node.filter.clone(),
        offset: node.offset,
    })
}

#[cfg(feature = "vector")]
pub(in crate::query::executor::streaming::plan::arena_builder) fn build_vector_lookup_spec(
    node: &crate::query::planning::plan::core::nodes::search::vector::data_access::VectorLookupNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    // LOOKUP VECTOR resolves to the same index location as SEARCH VECTOR and
    // executes through the identical search path; the query expression keeps
    // its semantics (converted here exactly like a search query vector).
    let query_vector = vector_query_to_vec(&node.query)?;
    Ok(VectorSpec::VectorLookup {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: node.space_id,
        index_name: node.index_name.clone(),
        query_vector,
        top_k: node.limit as u32,
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
    })
}

#[cfg(feature = "vector")]
pub(in crate::query::executor::streaming::plan::arena_builder) fn build_vector_match_spec(
    node: &crate::query::planning::plan::core::nodes::search::vector::data_access::VectorMatchNode,
    exec_ctx: &ExecutionContext,
) -> Result<VectorSpec, PlanBuildError> {
    let query_vector = vector_query_to_vec(&node.query)?;
    Ok(VectorSpec::VectorMatch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        pattern: node.pattern.clone(),
        field: node.field.clone(),
        query_vector,
        threshold: node.threshold,
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        space_id: node.space_id,
    })
}

#[cfg(feature = "vector")]
fn vector_query_to_vec(
    expr: &crate::query::parser::ast::vector::VectorQueryExpr,
) -> Result<Vec<f32>, PlanBuildError> {
    match expr.query_type {
        crate::query::parser::ast::vector::VectorQueryType::Vector => {
            let vec: Vec<f32> = serde_json::from_str(&expr.query_data).unwrap_or_default();
            Ok(vec)
        }
        crate::query::parser::ast::vector::VectorQueryType::Text => {
            // Text queries require an embedding service which is not wired
            // into the statement pipeline yet; fail loudly instead of silently
            // searching with an empty vector.
            Err(PlanBuildError::CapabilityUnavailable {
                capability: "text-embedding".to_string(),
                detail: format!(
                    "TEXT query for vector search is not supported yet: '{}'",
                    expr.query_data
                ),
            })
        }
        crate::query::parser::ast::vector::VectorQueryType::Parameter => {
            // Parameter-bound vectors are not bound at plan-build time yet;
            // fail loudly instead of silently searching with an empty vector.
            Err(PlanBuildError::CapabilityUnavailable {
                capability: "parameterized-vector-query".to_string(),
                detail: format!(
                    "PARAM query for vector search is not supported yet: '${}'",
                    expr.query_data
                ),
            })
        }
    }
}
fn fulltext_query_to_string(
    expr: &crate::query::parser::ast::fulltext::FulltextQueryExpr,
) -> String {
    match expr {
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Simple(text)
        | crate::query::parser::ast::fulltext::FulltextQueryExpr::Phrase(text)
        | crate::query::parser::ast::fulltext::FulltextQueryExpr::Prefix(text)
        | crate::query::parser::ast::fulltext::FulltextQueryExpr::Wildcard(text) => text.clone(),
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Field(field, text) => {
            format!("{field}:{text}")
        }
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Fuzzy(text, distance) => distance
            .map_or_else(
                || format!("{text}~"),
                |distance| format!("{text}~{distance}"),
            ),
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Boolean {
            must,
            should,
            must_not,
        } => must
            .iter()
            .map(|item| format!("+({})", fulltext_query_to_string(item)))
            .chain(
                should
                    .iter()
                    .map(|item| format!("({})", fulltext_query_to_string(item))),
            )
            .chain(
                must_not
                    .iter()
                    .map(|item| format!("-({})", fulltext_query_to_string(item))),
            )
            .collect::<Vec<_>>()
            .join(" "),
        crate::query::parser::ast::fulltext::FulltextQueryExpr::MultiField(fields) => fields
            .iter()
            .map(|(field, text)| format!("{field}:{text}"))
            .collect::<Vec<_>>()
            .join(" OR "),
        crate::query::parser::ast::fulltext::FulltextQueryExpr::Range {
            field,
            lower,
            upper,
            include_lower,
            include_upper,
        } => format!(
            "{field}:{}{} TO {}{}",
            if *include_lower { "[" } else { "{" },
            lower.as_deref().unwrap_or("*"),
            upper.as_deref().unwrap_or("*"),
            if *include_upper { "]" } else { "}" },
        ),
    }
}
