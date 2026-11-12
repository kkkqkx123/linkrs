use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::operators::spec::{
    DdlSpec, EdgeManageCommand, IndexManageCommand, PropertyRename, SpaceManageCommand, TagManageCommand
};

fn space_manage_to_command(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode,
) -> SpaceManageCommand {
    use crate::executor::streaming::operators::spec::SpaceManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode::*;
    match node {
        Create(n) => SpaceManageCommand::Create {
            space_name: n.info().space_name.clone(),
            vid_type: n.info().vid_type.clone(),
            comment: n.info().comment.clone(),
        },
        Drop(n) => SpaceManageCommand::Drop {
            space_name: n.space_name().to_string(),
            if_exists: n.if_exists(),
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
        Clear(n) => SpaceManageCommand::Clear {
            space_name: n.space_name().to_string(),
        },
        Checkpoint(_) => SpaceManageCommand::Checkpoint,
        ExportDatabase(n) => SpaceManageCommand::ExportDatabase {
            path: n.path().to_string(),
        },
        ImportDatabase(n) => SpaceManageCommand::ImportDatabase {
            path: n.path().to_string(),
        },
        AttachDatabase(n) => SpaceManageCommand::AttachDatabase {
            alias: n.alias().to_string(),
            path: n.path().to_string(),
            db_type: n.db_type().map(|s| s.to_string()),
        },
        DetachDatabase(n) => SpaceManageCommand::DetachDatabase {
            alias: n.alias().to_string(),
        },
        PurgeExpired(n) => SpaceManageCommand::PurgeExpired {
            space_name: n.space_name().to_string(),
            target: n.target().cloned(),
        },
    }
}

fn tag_manage_to_command(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::TagManageNode,
) -> TagManageCommand {
    use crate::executor::streaming::operators::spec::TagManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::TagManageNode::*;
    match node {
        Create(n) => {
            let info = n.info();
            TagManageCommand::Create {
                tag_name: info.tag_name.clone(),
                properties: info.properties.clone(),
                if_not_exists: info.if_not_exists,
                ttl_duration: info.ttl_duration,
                ttl_col: info.ttl_col.clone(),
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
                new_comment: info.new_comment.clone(),
            }
        }
        Desc(n) => TagManageCommand::Desc {
            tag_name: n.tag_name().to_string(),
        },
        Rename(n) => TagManageCommand::Rename {
            old_name: n.old_name().to_string(),
            new_name: n.new_name().to_string(),
        },
        Drop(n) => TagManageCommand::Drop {
            tag_names: n.tag_names().to_vec(),
            if_exists: n.if_exists(),
        },
        Show(_) => TagManageCommand::Show,
        ShowCreate(n) => TagManageCommand::ShowCreate {
            tag_name: n.tag_name().to_string(),
        },
    }
}

fn edge_manage_to_command(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode,
) -> EdgeManageCommand {
    use crate::executor::streaming::operators::spec::EdgeManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode::*;
    match node {
        Create(n) => {
            let info = n.info();
            EdgeManageCommand::Create {
                edge_name: info.edge_name.clone(),
                properties: info.properties.clone(),
                src_tag_name: info.src_tag_name.clone(),
                dst_tag_name: info.dst_tag_name.clone(),
                if_not_exists: info.if_not_exists,
                ttl_duration: info.ttl_duration,
                ttl_col: info.ttl_col.clone(),
            }
        }
        Alter(n) => {
            let info = n.info();
            EdgeManageCommand::Alter {
                edge_name: info.edge_name.clone(),
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
                new_comment: info.new_comment.clone(),
            }
        }
        Rename(n) => EdgeManageCommand::Rename {
            old_name: n.old_name().to_string(),
            new_name: n.new_name().to_string(),
        },
        UpdateEndpoints(n) => EdgeManageCommand::UpdateEndpoints {
            edge_name: n.edge_name().to_string(),
            src_tag_name: n.src_tag_name().to_string(),
            dst_tag_name: n.dst_tag_name().to_string(),
            clear_constraint: n.clear_constraint(),
        },
        Desc(n) => EdgeManageCommand::Desc {
            edge_name: n.edge_name().to_string(),
        },
        Drop(n) => EdgeManageCommand::Drop {
            edge_names: n.edge_names().to_vec(),
            if_exists: n.if_exists(),
        },
        Show(_) => EdgeManageCommand::Show,
        ShowCreate(n) => EdgeManageCommand::ShowCreate {
            edge_name: n.edge_name().to_string(),
        },
    }
}

fn index_manage_to_command(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode,
) -> IndexManageCommand {
    use crate::executor::streaming::operators::spec::IndexManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode::*;
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
            if_exists: n.if_exists(),
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
            if_exists: n.if_exists(),
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

pub(in crate::executor::streaming::plan::arena_builder) fn build_space_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::SpaceManage {
        command: space_manage_to_command(node),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_tag_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::TagManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::TagManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: tag_manage_to_command(node),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_edge_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::EdgeManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: edge_manage_to_command(node),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_index_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::IndexManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: index_manage_to_command(node),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_delete_index_spec(
    node: &crate::planning::plan::core::nodes::data_modification::delete_nodes::DeleteIndexNode,
    _exec_ctx: &ExecutionContext,
) -> Result<DdlSpec, PlanBuildError> {
    Ok(DdlSpec::DeleteIndex {
        space_name: node.info().space_name.clone(),
        index_name: node.info().index_name.clone(),
    })
}
