//! Definition of Schema change type

use crate::core::types::property::PropertyDef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SchemaChangeType {
    AddProperty,
    DropProperty,
    ModifyProperty,
    AddIndex,
    DropIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemaChange {
    pub change_type: SchemaChangeType,
    pub target: String,
    pub property: Option<PropertyDef>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaFieldChange {
    pub field_name: String,
    pub change_type: FieldChangeType,
    pub old_value: Option<PropertyDef>,
    pub new_value: Option<PropertyDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FieldChangeType {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaAlterOperation {
    pub space_name: String,
    pub target_type: AlterTargetType,
    pub target_name: String,
    pub field_changes: Vec<SchemaFieldChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlterTargetType {
    Tag,
    EdgeType,
}

impl SchemaAlterOperation {
    pub fn new_add_tag_field(space_name: String, tag_name: String, field: PropertyDef) -> Self {
        let field_name = field.name.clone();
        Self {
            space_name,
            target_type: AlterTargetType::Tag,
            target_name: tag_name,
            field_changes: vec![SchemaFieldChange {
                field_name,
                change_type: FieldChangeType::Added,
                old_value: None,
                new_value: Some(field),
            }],
        }
    }

    pub fn new_remove_tag_field(space_name: String, tag_name: String, field_name: String) -> Self {
        Self {
            space_name,
            target_type: AlterTargetType::Tag,
            target_name: tag_name,
            field_changes: vec![SchemaFieldChange {
                field_name,
                change_type: FieldChangeType::Removed,
                old_value: None,
                new_value: None,
            }],
        }
    }

    pub fn new_modify_tag_field(
        space_name: String,
        tag_name: String,
        old_field: PropertyDef,
        new_field: PropertyDef,
    ) -> Self {
        let field_name = old_field.name.clone();
        Self {
            space_name,
            target_type: AlterTargetType::Tag,
            target_name: tag_name,
            field_changes: vec![SchemaFieldChange {
                field_name,
                change_type: FieldChangeType::Modified,
                old_value: Some(old_field),
                new_value: Some(new_field),
            }],
        }
    }
}
