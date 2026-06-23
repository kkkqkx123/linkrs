//! Basic types of tags

use super::property::PropertyDef;
use super::schema_trait::SchemaInfo;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TagInfo {
    pub tag_id: u32,
    pub tag_name: String,
    pub properties: Vec<PropertyDef>,
    pub comment: Option<String>,
    pub ttl_duration: Option<i64>,
    pub ttl_col: Option<String>,
}

impl SchemaInfo for TagInfo {
    fn schema_id(&self) -> u32 {
        self.tag_id
    }

    fn schema_name(&self) -> &str {
        &self.tag_name
    }

    fn properties(&self) -> &[PropertyDef] {
        &self.properties
    }

    fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    fn ttl_duration(&self) -> Option<i64> {
        self.ttl_duration
    }

    fn ttl_col(&self) -> Option<&str> {
        self.ttl_col.as_deref()
    }

    fn set_schema_id(&mut self, id: u32) {
        self.tag_id = id;
    }

    fn set_properties(&mut self, properties: Vec<PropertyDef>) {
        self.properties = properties;
    }

    fn set_comment(&mut self, comment: Option<String>) {
        self.comment = comment;
    }

    fn set_ttl(&mut self, duration: Option<i64>, col: Option<String>) {
        self.ttl_duration = duration;
        self.ttl_col = col;
    }

    fn schema_type_name(&self) -> &'static str {
        "Tag"
    }

    fn is_tag(&self) -> bool {
        true
    }

    fn is_edge(&self) -> bool {
        false
    }
}

impl TagInfo {
    pub fn new(tag_name: String) -> Self {
        Self {
            tag_id: 0,
            tag_name,
            properties: Vec::new(),
            comment: None,
            ttl_duration: None,
            ttl_col: None,
        }
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
}

impl Default for TagInfo {
    fn default() -> Self {
        Self::new("default".to_string())
    }
}
