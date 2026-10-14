//! Basic Definitions of Edge Types

use super::property::PropertyDef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeStrategy {
    None,
    Single,
    #[default]
    Multiple,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeTypeInfo {
    pub edge_type_id: u32,
    pub edge_type_name: String,
    pub src_tag_name: String,
    pub dst_tag_name: String,
    pub properties: Vec<PropertyDef>,
    pub comment: Option<String>,
    pub ttl_duration: Option<i64>,
    pub ttl_col: Option<String>,
    #[serde(default)]
    pub oe_strategy: EdgeStrategy,
    #[serde(default)]
    pub ie_strategy: EdgeStrategy,
}

impl EdgeTypeInfo {
    pub fn new(edge_type_name: String) -> Self {
        Self {
            edge_type_id: 0,
            edge_type_name,
            src_tag_name: String::new(),
            dst_tag_name: String::new(),
            properties: Vec::new(),
            comment: None,
            ttl_duration: None,
            ttl_col: None,
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
        }
    }

    pub fn with_src_tag(mut self, src_tag_name: String) -> Self {
        self.src_tag_name = src_tag_name;
        self
    }

    pub fn with_dst_tag(mut self, dst_tag_name: String) -> Self {
        self.dst_tag_name = dst_tag_name;
        self
    }

    pub fn with_properties(mut self, properties: Vec<PropertyDef>) -> Self {
        self.properties = properties;
        self
    }

    pub fn with_comment(mut self, comment: Option<String>) -> Self {
        self.comment = comment;
        self
    }

    pub fn with_ttl(mut self, duration: Option<i64>, col: Option<String>) -> Self {
        self.ttl_duration = duration;
        self.ttl_col = col;
        self
    }

    pub fn with_strategies(mut self, oe_strategy: EdgeStrategy, ie_strategy: EdgeStrategy) -> Self {
        self.oe_strategy = oe_strategy;
        self.ie_strategy = ie_strategy;
        self
    }
}

impl Default for EdgeTypeInfo {
    fn default() -> Self {
        Self::new("default".to_string())
    }
}
