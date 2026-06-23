//! Implementation of the index management node
//!
//! Provide definitions for the planning nodes related to index management.

use crate::define_plan_node;

define_plan_node! {
    pub struct CreateTagIndexNode {
        info: IndexManageInfo,
    }
    manage_enum: IndexManageNode::CreateTagIndex as IndexManage
    input: ZeroInputNode
}

impl CreateTagIndexNode {
    pub fn new(id: i64, info: IndexManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn info(&self) -> &IndexManageInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.info.index_name
    }
}

define_plan_node! {
    pub struct DropTagIndexNode {
        space_name: String,
        index_name: String,
    }
    manage_enum: IndexManageNode::DropTagIndex as IndexManage
    input: ZeroInputNode
}

impl DropTagIndexNode {
    pub fn new(id: i64, space_name: String, index_name: String) -> Self {
        Self {
            id,
            space_name,
            index_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }
}

define_plan_node! {
    pub struct DescTagIndexNode {
        space_name: String,
        index_name: String,
    }
    manage_enum: IndexManageNode::DescTagIndex as IndexManage
    input: ZeroInputNode
}

impl DescTagIndexNode {
    pub fn new(id: i64, space_name: String, index_name: String) -> Self {
        Self {
            id,
            space_name,
            index_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }
}

define_plan_node! {
    pub struct ShowTagIndexesNode {
        space_name: String,
    }
    manage_enum: IndexManageNode::ShowTagIndexes as IndexManage
    input: ZeroInputNode
}

impl ShowTagIndexesNode {
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

define_plan_node! {
    pub struct CreateEdgeIndexNode {
        info: IndexManageInfo,
    }
    manage_enum: IndexManageNode::CreateEdgeIndex as IndexManage
    input: ZeroInputNode
}

impl CreateEdgeIndexNode {
    pub fn new(id: i64, info: IndexManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn info(&self) -> &IndexManageInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.info.index_name
    }
}

define_plan_node! {
    pub struct DropEdgeIndexNode {
        space_name: String,
        index_name: String,
    }
    manage_enum: IndexManageNode::DropEdgeIndex as IndexManage
    input: ZeroInputNode
}

impl DropEdgeIndexNode {
    pub fn new(id: i64, space_name: String, index_name: String) -> Self {
        Self {
            id,
            space_name,
            index_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }
}

define_plan_node! {
    pub struct DescEdgeIndexNode {
        space_name: String,
        index_name: String,
    }
    manage_enum: IndexManageNode::DescEdgeIndex as IndexManage
    input: ZeroInputNode
}

impl DescEdgeIndexNode {
    pub fn new(id: i64, space_name: String, index_name: String) -> Self {
        Self {
            id,
            space_name,
            index_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }
}

define_plan_node! {
    pub struct ShowEdgeIndexesNode {
        space_name: String,
    }
    manage_enum: IndexManageNode::ShowEdgeIndexes as IndexManage
    input: ZeroInputNode
}

impl ShowEdgeIndexesNode {
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

define_plan_node! {
    pub struct RebuildTagIndexNode {
        space_name: String,
        index_name: String,
    }
    manage_enum: IndexManageNode::RebuildTagIndex as IndexManage
    input: ZeroInputNode
}

impl RebuildTagIndexNode {
    pub fn new(id: i64, space_name: String, index_name: String) -> Self {
        Self {
            id,
            space_name,
            index_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }
}

define_plan_node! {
    pub struct RebuildEdgeIndexNode {
        space_name: String,
        index_name: String,
    }
    manage_enum: IndexManageNode::RebuildEdgeIndex as IndexManage
    input: ZeroInputNode
}

impl RebuildEdgeIndexNode {
    pub fn new(id: i64, space_name: String, index_name: String) -> Self {
        Self {
            id,
            space_name,
            index_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }
}

/// Index management information
#[derive(Debug, Clone)]
pub struct IndexManageInfo {
    pub space_name: String,
    pub index_name: String,
    pub target_type: String,
    pub target_name: String,
    pub properties: Vec<String>,
}

impl IndexManageInfo {
    pub fn new(space_name: String, index_name: String, target_type: String) -> Self {
        Self {
            space_name,
            index_name,
            target_type,
            target_name: String::new(),
            properties: Vec::new(),
        }
    }

    pub fn with_target_name(mut self, target_name: String) -> Self {
        self.target_name = target_name;
        self
    }

    pub fn with_properties(mut self, properties: Vec<String>) -> Self {
        self.properties = properties;
        self
    }
}

define_plan_node! {
    pub struct ShowCreateIndexNode {
        space_name: String,
        index_name: String,
    }
    manage_enum: IndexManageNode::ShowCreateIndex as IndexManage
    input: ZeroInputNode
}

impl ShowCreateIndexNode {
    pub fn new(id: i64, space_name: String, index_name: String) -> Self {
        Self {
            id,
            space_name,
            index_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }
}

define_plan_node! {
    pub struct ShowIndexesNode {
        space_name: String,
    }
    manage_enum: IndexManageNode::ShowIndexes as IndexManage
    input: ZeroInputNode
}

impl ShowIndexesNode {
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
