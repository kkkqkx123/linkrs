//! Implementation of the edge type management node
//!
//! Provide definitions for the planning nodes related to edge type management.

use crate::define_plan_node;
use graphdb_core::types::PropertyDef;

define_plan_node! {
    pub struct CreateEdgeNode {
        info: EdgeManageInfo,
    }
    manage_enum: EdgeManageNode::Create as EdgeManage
    input: ZeroInputNode
}

impl CreateEdgeNode {
    pub fn new(id: i64, info: EdgeManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &EdgeManageInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn edge_name(&self) -> &str {
        &self.info.edge_name
    }
}

define_plan_node! {
    pub struct AlterEdgeNode {
        info: EdgeAlterInfo,
    }
    manage_enum: EdgeManageNode::Alter as EdgeManage
    input: ZeroInputNode
}

impl AlterEdgeNode {
    pub fn new(id: i64, info: EdgeAlterInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &EdgeAlterInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn edge_name(&self) -> &str {
        &self.info.edge_name
    }
}

define_plan_node! {
    pub struct DescEdgeNode {
        space_name: String,
        edge_name: String,
    }
    manage_enum: EdgeManageNode::Desc as EdgeManage
    input: ZeroInputNode
}

impl DescEdgeNode {
    pub fn new(id: i64, space_name: String, edge_name: String) -> Self {
        Self {
            id,
            space_name,
            edge_name,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn edge_name(&self) -> &str {
        &self.edge_name
    }
}

define_plan_node! {
    pub struct DropEdgeNode {
        space_name: String,
        edge_name: String,
        if_exists: bool,
    }
    manage_enum: EdgeManageNode::Drop as EdgeManage
    input: ZeroInputNode
}

impl DropEdgeNode {
    pub fn new(id: i64, space_name: String, edge_name: String) -> Self {
        Self {
            id,
            space_name,
            edge_name,
            if_exists: false,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn edge_name(&self) -> &str {
        &self.edge_name
    }

    pub fn if_exists(&self) -> bool {
        self.if_exists
    }
}

define_plan_node! {
    pub struct ShowEdgesNode {
        space_name: String,
    }
    manage_enum: EdgeManageNode::Show as EdgeManage
    input: ZeroInputNode
}

impl ShowEdgesNode {
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
    pub struct ShowCreateEdgeNode {
        space_name: String,
        edge_name: String,
    }
    manage_enum: EdgeManageNode::ShowCreate as EdgeManage
    input: ZeroInputNode
}

impl ShowCreateEdgeNode {
    pub fn new(id: i64, space_name: String, edge_name: String) -> Self {
        Self {
            id,
            space_name,
            edge_name,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn edge_name(&self) -> &str {
        &self.edge_name
    }
}

define_plan_node! {
    pub struct RenameEdgeNode {
        space_name: String,
        old_name: String,
        new_name: String,
    }
    manage_enum: EdgeManageNode::Rename as EdgeManage
    input: ZeroInputNode
}

impl RenameEdgeNode {
    pub fn new(id: i64, space_name: String, old_name: String, new_name: String) -> Self {
        Self {
            id,
            space_name,
            old_name,
            new_name,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn old_name(&self) -> &str {
        &self.old_name
    }

    pub fn new_name(&self) -> &str {
        &self.new_name
    }
}

define_plan_node! {
    pub struct UpdateEdgeEndpointsNode {
        space_name: String,
        edge_name: String,
        src_tag_name: String,
        dst_tag_name: String,
        clear_constraint: bool,
    }
    manage_enum: EdgeManageNode::UpdateEndpoints as EdgeManage
    input: ZeroInputNode
}

impl UpdateEdgeEndpointsNode {
    pub fn new(
        id: i64,
        space_name: String,
        edge_name: String,
        src_tag_name: String,
        dst_tag_name: String,
        clear_constraint: bool,
    ) -> Self {
        Self {
            id,
            space_name,
            edge_name,
            src_tag_name,
            dst_tag_name,
            clear_constraint,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn edge_name(&self) -> &str {
        &self.edge_name
    }

    pub fn src_tag_name(&self) -> &str {
        &self.src_tag_name
    }

    pub fn dst_tag_name(&self) -> &str {
        &self.dst_tag_name
    }

    pub fn clear_constraint(&self) -> bool {
        self.clear_constraint
    }
}

/// Edge Type Management Information
#[derive(Debug, Clone)]
pub struct EdgeManageInfo {
    pub space_name: String,
    pub edge_name: String,
    pub src_tag_name: Option<String>,
    pub dst_tag_name: Option<String>,
    pub properties: Vec<PropertyDef>,
    pub if_not_exists: bool,
}

impl EdgeManageInfo {
    pub fn new(space_name: String, edge_name: String) -> Self {
        Self {
            space_name,
            edge_name,
            src_tag_name: None,
            dst_tag_name: None,
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

    pub fn with_src_dst_tags(mut self, src_tag_name: String, dst_tag_name: String) -> Self {
        self.src_tag_name = Some(src_tag_name);
        self.dst_tag_name = Some(dst_tag_name);
        self
    }
}

/// Information on changes to the border type
#[derive(Debug, Clone)]
pub struct EdgeAlterInfo {
    pub space_name: String,
    pub edge_name: String,
    pub additions: Vec<PropertyDef>,
    pub deletions: Vec<String>,
}

impl EdgeAlterInfo {
    pub fn new(space_name: String, edge_name: String) -> Self {
        Self {
            space_name,
            edge_name,
            additions: Vec::new(),
            deletions: Vec::new(),
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
}
