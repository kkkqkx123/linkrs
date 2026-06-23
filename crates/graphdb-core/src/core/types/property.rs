//! Attribute Definition Base Type

use super::property_trait::PropertyTypeTrait;
use crate::core::{DataType, Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PropertyDef {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
    pub default: Option<Value>,
    pub comment: Option<String>,
}

impl PropertyTypeTrait for PropertyDef {
    fn name(&self) -> &str {
        &self.name
    }

    fn data_type(&self) -> &DataType {
        &self.data_type
    }

    fn is_nullable(&self) -> bool {
        self.nullable
    }

    fn default_value(&self) -> Option<&Value> {
        self.default.as_ref()
    }

    fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    fn set_name(&mut self, name: String) {
        self.name = name;
    }

    fn set_data_type(&mut self, data_type: DataType) {
        self.data_type = data_type;
    }

    fn set_nullable(&mut self, nullable: bool) {
        self.nullable = nullable;
    }

    fn set_default_value(&mut self, default: Option<Value>) {
        self.default = default;
    }

    fn set_comment(&mut self, comment: Option<String>) {
        self.comment = comment;
    }

    fn property_type_name(&self) -> &'static str {
        "PropertyDef"
    }
}

impl PropertyDef {
    pub fn new(name: String, data_type: DataType) -> Self {
        Self {
            name,
            data_type,
            nullable: false,
            default: None,
            comment: None,
        }
    }

    pub fn with_nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    pub fn with_default(mut self, default: Option<Value>) -> Self {
        self.default = default;
        self
    }

    pub fn with_comment(mut self, comment: Option<String>) -> Self {
        self.comment = comment;
        self
    }
}

impl Default for PropertyDef {
    fn default() -> Self {
        Self::new("default".to_string(), DataType::String)
    }
}
