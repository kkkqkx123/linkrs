//! Vertex Storage Module
//!
//! Provides columnar storage for vertex data with MVCC timestamp support.
//!
//! ## Components
//!
//! - `VertexTable`: Main vertex storage with columnar layout
//! - `IdIndexer`: External ID to internal ID mapping
//! - `ColumnStore`: Columnar property storage
//! - `VertexTimestamp`: MVCC timestamp tracking for vertices

pub mod column_store;
pub mod id_indexer;
pub mod vertex_table;
pub mod vertex_timestamp;

use crate::storage::types::StoragePropertyDef;

pub use column_store::ColumnStore;
pub use id_indexer::{IdIndexer, IdKey};
pub use vertex_table::VertexTable;
pub use vertex_timestamp::VertexTimestamp;

use crate::core::vertex_edge_path::Tag;
use crate::core::Value;

pub use crate::core::types::{LabelId, Timestamp, VertexId, INVALID_TIMESTAMP, MAX_TIMESTAMP};

#[derive(Debug, Clone)]
pub struct VertexRecord {
    pub vid: VertexId,
    pub internal_id: u32,
    pub properties: Vec<(String, Value)>,
}

impl From<&VertexRecord> for crate::core::Vertex {
    fn from(record: &VertexRecord) -> Self {
        let properties: std::collections::HashMap<String, Value> =
            record.properties.iter().cloned().collect();

        crate::core::Vertex {
            vid: record.vid,
            id: record.internal_id as i64,
            tags: vec![Tag {
                name: String::new(),
                properties: properties.clone(),
            }],
            properties,
        }
    }
}

impl VertexRecord {
    pub fn into_vertex_with_tag(self, tag_name: &str) -> crate::core::Vertex {
        let properties: std::collections::HashMap<String, Value> =
            self.properties.into_iter().collect();

        crate::core::Vertex {
            vid: self.vid,
            id: self.internal_id as i64,
            tags: vec![Tag {
                name: tag_name.to_string(),
                properties: properties.clone(),
            }],
            properties,
        }
    }
}

/// Default schema version (1) for new schemas and deserialization
fn default_schema_version() -> u64 {
    1
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VertexSchema {
    pub label_id: LabelId,
    pub label_name: String,
    pub properties: Vec<StoragePropertyDef>,
    pub primary_key_index: usize,
    /// Schema version for migration tracking
    #[serde(default = "default_schema_version")]
    pub schema_version: u64,
}

impl VertexSchema {
    /// Validate that the loaded schema matches the expected version.
    /// Returns Ok(()) if valid, Err with description if there are issues.
    ///
    /// Note: This method must be called explicitly by callers to enforce version checking.
    pub fn validate(&self, expected_version: u64) -> Result<(), String> {
        if self.schema_version != expected_version {
            return Err(format!(
                "Schema version mismatch: expected {}, got {}",
                expected_version, self.schema_version
            ));
        }

        // Digest validation reserved for future use
        Ok(())
    }

    /// Validate schema at creation time
    /// Ensures primary key exists and has a valid type for use as a key
    pub fn validate_on_creation(&self) -> Result<(), String> {
        // Validate primary key index
        if self.properties.is_empty() {
            return Err("Schema must have at least one property".to_string());
        }

        if self.primary_key_index >= self.properties.len() {
            return Err(format!(
                "Invalid primary key index: {} >= property count {}",
                self.primary_key_index,
                self.properties.len()
            ));
        }

        let primary_key_prop = &self.properties[self.primary_key_index];

        // Validate primary key is not nullable
        if primary_key_prop.nullable {
            return Err(format!(
                "Primary key '{}' cannot be nullable",
                primary_key_prop.name
            ));
        }

        // Validate primary key type is suitable for keys (must be comparable and hashable)
        Self::validate_key_type(&primary_key_prop.data_type, &primary_key_prop.name)?;

        // Check if property name is valid (non-empty, valid identifier)
        if primary_key_prop.name.is_empty() {
            return Err("Primary key name cannot be empty".to_string());
        }

        Self::validate_identifier(&primary_key_prop.name)?;

        // Validate all property names are unique and valid
        let mut seen_names = std::collections::HashSet::new();
        for prop in &self.properties {
            if !seen_names.insert(&prop.name) {
                return Err(format!("Duplicate property name: '{}'", prop.name));
            }

            // Validate each property name
            if prop.name.is_empty() {
                return Err("Property name cannot be empty".to_string());
            }

            Self::validate_identifier(&prop.name)?;

            // Validate property data types are not Empty or Null
            Self::validate_property_type(&prop.data_type, &prop.name)?;
        }

        Ok(())
    }

    /// Validate that an identifier (name) follows valid rules
    /// Must start with letter or underscore, contain only alphanumeric + underscore
    fn validate_identifier(name: &str) -> Result<(), String> {
        let first_char = match name.chars().next() {
            Some(c) => c,
            None => return Err("Identifier cannot be empty".to_string()),
        };

        if !first_char.is_ascii_alphabetic() && first_char != '_' {
            return Err(format!(
                "Identifier '{}' must start with ASCII letter or underscore, got '{}'",
                name, first_char
            ));
        }

        // Check all characters are ASCII alphanumeric or underscore
        for (i, c) in name.chars().enumerate() {
            if !c.is_ascii_alphanumeric() && c != '_' {
                return Err(format!(
                    "Identifier '{}' contains invalid character '{}' at position {}. \
                     Only ASCII letters, digits, and underscores are allowed.",
                    name, c, i
                ));
            }
        }

        Ok(())
    }

    /// Validate that a data type is suitable for use as a primary key
    /// Primary keys must be:
    /// - Comparable (support <, >, ==)
    /// - Hashable
    /// - Not composite types
    fn validate_key_type(data_type: &crate::core::DataType, prop_name: &str) -> Result<(), String> {
        use crate::core::DataType;

        // Valid key types - scalar, comparable types
        let valid_key_types = [
            DataType::Bool,
            DataType::SmallInt,
            DataType::Int,
            DataType::BigInt,
            DataType::Float,
            DataType::Double,
            DataType::Decimal128,
            DataType::String,
            DataType::Date,
            DataType::Time,
            DataType::DateTime,
            DataType::Timestamp,
            DataType::VID,
            DataType::Uuid,
        ];

        for valid_type in &valid_key_types {
            if std::mem::discriminant(data_type) == std::mem::discriminant(valid_type) {
                return Ok(());
            }
        }

        // If we get here, the type is not valid for keys
        Err(format!(
            "Primary key '{}' has invalid type '{:?}'. \
             Allowed types: Bool, SmallInt, Int, BigInt, Float, Double, Decimal128, \
             String, Date, Time, DateTime, Timestamp, VID, Uuid",
            prop_name, data_type
        ))
    }

    /// Validate that a property data type is allowed
    /// Rejects Empty and Null types which don't make sense as properties
    fn validate_property_type(data_type: &crate::core::DataType, prop_name: &str) -> Result<(), String> {
        use crate::core::DataType;

        match data_type {
            DataType::Empty => {
                return Err(format!(
                    "Property '{}' cannot have type Empty - properties must have valid types",
                    prop_name
                ));
            }
            DataType::Null => {
                return Err(format!(
                    "Property '{}' cannot have type Null - use nullable=true instead",
                    prop_name
                ));
            }
            _ => Ok(()),
        }
    }

    /// Increment schema version when schema changes.
    pub fn increment_version(&mut self) {
        self.schema_version += 1;
    }
}
