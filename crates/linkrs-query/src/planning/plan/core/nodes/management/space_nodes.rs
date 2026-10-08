//! Implementation of space management node
//!
//! Provide definitions of planning nodes related to graph space management.

use crate::define_plan_node;

define_plan_node! {
    pub struct CreateSpaceNode {
        info: SpaceManageInfo,
    }
    manage_enum: SpaceManageNode::Create as SpaceManage
    input: ZeroInputNode
}

impl CreateSpaceNode {
    pub fn new(id: i64, info: SpaceManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &SpaceManageInfo {
        &self.info
    }
}

define_plan_node! {
    pub struct DropSpaceNode {
        space_name: String,
    }
    manage_enum: SpaceManageNode::Drop as SpaceManage
    input: ZeroInputNode
}

impl DropSpaceNode {
    pub fn new(id: i64, space_name: String) -> Self {
        Self {
            id,
            space_name,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }
}

define_plan_node! {
    pub struct DescSpaceNode {
        space_name: String,
    }
    manage_enum: SpaceManageNode::Desc as SpaceManage
    input: ZeroInputNode
}

impl DescSpaceNode {
    pub fn new(id: i64, space_name: String) -> Self {
        Self {
            id,
            space_name,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }
}

define_plan_node! {
    pub struct ShowSpacesNode {
    }
    manage_enum: SpaceManageNode::Show as SpaceManage
    input: ZeroInputNode
}

impl ShowSpacesNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }
}

define_plan_node! {
    pub struct SwitchSpaceNode {
        space_name: String,
    }
    manage_enum: SpaceManageNode::Switch as SpaceManage
    input: ZeroInputNode
}

impl SwitchSpaceNode {
    pub fn new(id: i64, space_name: String) -> Self {
        Self {
            id,
            space_name,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }
}

define_plan_node! {
    pub struct AlterSpaceNode {
        space_name: String,
        options: Vec<SpaceAlterOption>,
    }
    manage_enum: SpaceManageNode::Alter as SpaceManage
    input: ZeroInputNode
}

impl AlterSpaceNode {
    pub fn new(id: i64, space_name: String, options: Vec<SpaceAlterOption>) -> Self {
        Self {
            id,
            space_name,
            options,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn options(&self) -> &[SpaceAlterOption] {
        &self.options
    }
}

define_plan_node! {
    pub struct ClearSpaceNode {
        space_name: String,
    }
    manage_enum: SpaceManageNode::Clear as SpaceManage
    input: ZeroInputNode
}

impl ClearSpaceNode {
    pub fn new(id: i64, space_name: String) -> Self {
        Self {
            id,
            space_name,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }
}

/// Space modification options
#[derive(Debug, Clone)]
pub enum SpaceAlterOption {
    Comment(String),
}

define_plan_node! {
    pub struct ShowCreateSpaceNode {
        space_name: String,
    }
    manage_enum: SpaceManageNode::ShowCreate as SpaceManage
    input: ZeroInputNode
}

impl ShowCreateSpaceNode {
    pub fn new(id: i64, space_name: String) -> Self {
        Self {
            id,
            space_name,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }
}

/// Space management information
#[derive(Debug, Clone)]
pub struct SpaceManageInfo {
    pub space_name: String,
    pub vid_type: String,
}

impl SpaceManageInfo {
    pub fn new(space_name: String) -> Self {
        Self {
            space_name,
            vid_type: "FIXED_STRING(32)".to_string(),
        }
    }

    pub fn with_vid_type(mut self, vid_type: String) -> Self {
        self.vid_type = vid_type;
        self
    }
}

use crate::parser::ast::stmt::{CommentTarget, ExportOption};

define_plan_node! {
    pub struct CommentOnNode {
        target: CommentTarget,
        comment: String,
    }
    manage_enum: SpaceManageNode::CommentOn as SpaceManage
    input: ZeroInputNode
}

impl CommentOnNode {
    pub fn new(id: i64, target: CommentTarget, comment: String) -> Self {
        Self {
            id,
            target,
            comment,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn target(&self) -> &CommentTarget {
        &self.target
    }

    pub fn comment(&self) -> &str {
        &self.comment
    }
}

define_plan_node! {
    pub struct CheckpointNode {
    }
    manage_enum: SpaceManageNode::Checkpoint as SpaceManage
    input: ZeroInputNode
}

impl CheckpointNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }
}

define_plan_node! {
    pub struct ExportDatabaseNode {
        path: String,
        options: Vec<ExportOption>,
    }
    manage_enum: SpaceManageNode::ExportDatabase as SpaceManage
    input: ZeroInputNode
}

impl ExportDatabaseNode {
    pub fn new(id: i64, path: String, options: Vec<ExportOption>) -> Self {
        Self {
            id,
            path,
            options,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn options(&self) -> &[ExportOption] {
        &self.options
    }
}

define_plan_node! {
    pub struct ImportDatabaseNode {
        path: String,
    }
    manage_enum: SpaceManageNode::ImportDatabase as SpaceManage
    input: ZeroInputNode
}

impl ImportDatabaseNode {
    pub fn new(id: i64, path: String) -> Self {
        Self {
            id,
            path,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

define_plan_node! {
    pub struct AttachDatabaseNode {
        path: String,
        alias: String,
        db_type: Option<String>,
        options: Vec<(String, String)>,
    }
    manage_enum: SpaceManageNode::AttachDatabase as SpaceManage
    input: ZeroInputNode
}

impl AttachDatabaseNode {
    pub fn new(
        id: i64,
        path: String,
        alias: String,
        db_type: Option<String>,
        options: Vec<(String, String)>,
    ) -> Self {
        Self {
            id,
            path,
            alias,
            db_type,
            options,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn alias(&self) -> &str {
        &self.alias
    }
    pub fn db_type(&self) -> Option<&str> {
        self.db_type.as_deref()
    }
    pub fn options(&self) -> &[(String, String)] {
        &self.options
    }
}

define_plan_node! {
    pub struct DetachDatabaseNode {
        alias: String,
    }
    manage_enum: SpaceManageNode::DetachDatabase as SpaceManage
    input: ZeroInputNode
}

impl DetachDatabaseNode {
    pub fn new(id: i64, alias: String) -> Self {
        Self {
            id,
            alias,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }
}
