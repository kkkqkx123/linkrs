//! Implementation of the Tag Management Node
//!
//! Provide definitions for the planning nodes related to label management.

use crate::core::types::PropertyDef;
use crate::define_plan_node;
use crate::query::parser::ast::stmt::PropertyChange;

define_plan_node! {
    pub struct CreateTagNode {
        info: TagManageInfo,
    }
    manage_enum: TagManageNode::Create as TagManage
    input: ZeroInputNode
}

impl CreateTagNode {
    pub fn new(id: i64, info: TagManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn info(&self) -> &TagManageInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn tag_name(&self) -> &str {
        &self.info.tag_name
    }
}

define_plan_node! {
    pub struct AlterTagNode {
        info: TagAlterInfo,
    }
    manage_enum: TagManageNode::Alter as TagManage
    input: ZeroInputNode
}

impl AlterTagNode {
    pub fn new(id: i64, info: TagAlterInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn info(&self) -> &TagAlterInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn tag_name(&self) -> &str {
        &self.info.tag_name
    }
}

define_plan_node! {
    pub struct DescTagNode {
        space_name: String,
        tag_name: String,
    }
    manage_enum: TagManageNode::Desc as TagManage
    input: ZeroInputNode
}

impl DescTagNode {
    pub fn new(id: i64, space_name: String, tag_name: String) -> Self {
        Self {
            id,
            space_name,
            tag_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn tag_name(&self) -> &str {
        &self.tag_name
    }
}

define_plan_node! {
    pub struct DropTagNode {
        space_name: String,
        tag_name: String,
        if_exists: bool,
    }
    manage_enum: TagManageNode::Drop as TagManage
    input: ZeroInputNode
}

impl DropTagNode {
    pub fn new(id: i64, space_name: String, tag_name: String) -> Self {
        Self {
            id,
            space_name,
            tag_name,
            if_exists: false,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn tag_name(&self) -> &str {
        &self.tag_name
    }

    pub fn if_exists(&self) -> bool {
        self.if_exists
    }
}

define_plan_node! {
    pub struct ShowTagsNode {
        space_name: String,
    }
    manage_enum: TagManageNode::Show as TagManage
    input: ZeroInputNode
}

impl ShowTagsNode {
    pub fn new(id: i64, space_name: String) -> Self {
        Self {
            id,
            space_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }
}

/// Tag management information
#[derive(Debug, Clone)]
pub struct TagManageInfo {
    pub space_name: String,
    pub tag_name: String,
    pub properties: Vec<PropertyDef>,
    pub if_not_exists: bool,
}

impl TagManageInfo {
    pub fn new(space_name: String, tag_name: String) -> Self {
        Self {
            space_name,
            tag_name,
            properties: Vec::new(),
            if_not_exists: false,
        }
    }

    pub fn with_properties(mut self, properties: Vec<PropertyDef>) -> Self {
        self.properties = properties;
        self
    }

    pub fn with_if_not_exists(mut self, if_not_exists: bool) -> Self {
        self.if_not_exists = if_not_exists;
        self
    }
}

/// Tag modification information
#[derive(Debug, Clone)]
pub struct TagAlterInfo {
    pub space_name: String,
    pub tag_name: String,
    pub additions: Vec<PropertyDef>,
    pub deletions: Vec<String>,
    pub changes: Vec<PropertyChange>,
}

impl TagAlterInfo {
    pub fn new(space_name: String, tag_name: String) -> Self {
        Self {
            space_name,
            tag_name,
            additions: Vec::new(),
            deletions: Vec::new(),
            changes: Vec::new(),
        }
    }

    pub fn with_additions(mut self, additions: Vec<PropertyDef>) -> Self {
        self.additions = additions;
        self
    }

    pub fn with_deletions(mut self, deletions: Vec<String>) -> Self {
        self.deletions = deletions;
        self
    }

    pub fn with_changes(mut self, changes: Vec<PropertyChange>) -> Self {
        self.changes = changes;
        self
    }
}

define_plan_node! {
    pub struct ShowCreateTagNode {
        space_name: String,
        tag_name: String,
    }
    manage_enum: TagManageNode::ShowCreate as TagManage
    input: ZeroInputNode
}

impl ShowCreateTagNode {
    pub fn new(id: i64, space_name: String, tag_name: String) -> Self {
        Self {
            id,
            space_name,
            tag_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn tag_name(&self) -> &str {
        &self.tag_name
    }
}
