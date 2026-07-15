use crate::core::types::{DataType, EdgeTypeInfo, PropertyDef, TagInfo};
use crate::core::Value;
use crate::query::validator::error::{ValidationError as CoreValidationError, ValidationErrorType};

use super::schema_lookup::SchemaValidator;

type EdgeTypeAutoCreateDef = (String, String, String, Vec<(String, Value)>);

pub struct AutoCreateEdgeTypeParams<'a> {
    pub space_name: &'a str,
    pub edge_type_name: &'a str,
    pub src_tag_name: &'a str,
    pub dst_tag_name: &'a str,
    pub properties: &'a [(String, Value)],
}

pub struct AutoCreateMissingEdgeTypesParam<'a> {
    pub space_name: &'a str,
    pub edge_types: &'a [AutoCreateEdgeTypeParams<'a>],
}

impl SchemaValidator {
    pub fn auto_create_tag(
        &self,
        space_name: &str,
        tag_name: &str,
        properties: &[(String, Value)],
    ) -> Result<TagInfo, CoreValidationError> {
        if let Some(existing) = self
            .get_schema_manager()
            .get_tag(space_name, tag_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Tag: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })?
        {
            return Ok(existing);
        }

        let mut prop_defs = Vec::new();
        for (prop_name, value) in properties {
            let data_type = Self::infer_data_type(value);
            let prop_def = PropertyDef::new(prop_name.clone(), data_type).with_nullable(true);
            prop_defs.push(prop_def);
        }

        let tag_info = TagInfo {
            tag_id: 0,
            tag_name: tag_name.to_string(),
            properties: prop_defs,
            comment: Some("Auto-created for Cypher CREATE".to_string()),
            ttl_duration: None,
            ttl_col: None,
        };

        self.get_schema_manager()
            .create_tag(space_name, &tag_info)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Create Tag '{}' failed: {}", tag_name, e),
                    ValidationErrorType::SemanticError,
                )
            })?;

        Ok(tag_info)
    }

    pub fn auto_create_edge_type(
        &self,
        space_name: &str,
        edge_type_name: &str,
        src_tag_name: &str,
        dst_tag_name: &str,
        properties: &[(String, Value)],
    ) -> Result<EdgeTypeInfo, CoreValidationError> {
        if let Some(existing) = self
            .get_schema_manager()
            .get_edge_type(space_name, edge_type_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Edge Type: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })?
        {
            return Ok(existing);
        }

        let mut prop_defs = Vec::new();
        for (prop_name, value) in properties {
            let data_type = Self::infer_data_type(value);
            let prop_def = PropertyDef::new(prop_name.clone(), data_type).with_nullable(true);
            prop_defs.push(prop_def);
        }

        let edge_info = EdgeTypeInfo {
            edge_type_id: 0,
            edge_type_name: edge_type_name.to_string(),
            src_tag_name: src_tag_name.to_string(),
            dst_tag_name: dst_tag_name.to_string(),
            properties: prop_defs,
            comment: Some("Auto-created for Cypher CREATE".to_string()),
            ttl_duration: None,
            ttl_col: None,
            oe_strategy: crate::core::types::EdgeStrategy::Multiple,
            ie_strategy: crate::core::types::EdgeStrategy::Multiple,
        };

        self.get_schema_manager()
            .create_edge_type(space_name, &edge_info)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Create Edge Type '{}' failed: {}", edge_type_name, e),
                    ValidationErrorType::SemanticError,
                )
            })?;

        Ok(edge_info)
    }

    fn infer_data_type(value: &Value) -> DataType {
        match value {
            Value::Null(_) => DataType::String,
            Value::Bool(_) => DataType::Bool,
            Value::SmallInt(_) => DataType::SmallInt,
            Value::Int(_) => DataType::Int,
            Value::BigInt(_) => DataType::BigInt,
            Value::Float(_) => DataType::Float,
            Value::Double(_) => DataType::Double,
            Value::String(s) => {
                if s.len() <= 256 {
                    DataType::FixedString(s.len().max(32))
                } else {
                    DataType::String
                }
            }
            Value::List(_) => DataType::List,
            Value::Map(_) => DataType::Map,
            Value::Date(_) => DataType::Date,
            Value::DateTime(_) => DataType::DateTime,
            _ => DataType::String,
        }
    }

    pub fn auto_create_missing_tags(
        &self,
        space_name: &str,
        tags: &[(String, Vec<(String, Value)>)],
    ) -> Result<Vec<TagInfo>, CoreValidationError> {
        let mut created = Vec::new();
        for (tag_name, properties) in tags {
            let tag_info = self.auto_create_tag(space_name, tag_name, properties)?;
            created.push(tag_info);
        }
        Ok(created)
    }

    pub fn auto_create_missing_edge_types(
        &self,
        space_name: &str,
        edge_types: &[EdgeTypeAutoCreateDef],
    ) -> Result<Vec<EdgeTypeInfo>, CoreValidationError> {
        let mut created = Vec::new();
        for (edge_type_name, src_tag_name, dst_tag_name, properties) in edge_types {
            let edge_info = self.auto_create_edge_type(
                space_name,
                edge_type_name,
                src_tag_name,
                dst_tag_name,
                properties,
            )?;
            created.push(edge_info);
        }
        Ok(created)
    }
}
