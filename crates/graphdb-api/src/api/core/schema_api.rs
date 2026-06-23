//! Schema Manipulation API - Core Layer
//!
//! Provides transport layer-independent Schema management capabilities

use crate::api::core::{CoreError, CoreResult, IndexTarget, PropertyDef, SpaceConfig};
use crate::core::types::{
    EdgeTypeInfo, Index, IndexField, IndexStatus, IndexType, SpaceInfo, TagInfo,
};
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Schema Manipulation API - Core Layer
pub struct SchemaApi<S: StorageClient> {
    storage: Arc<RwLock<S>>,
}

impl<S: StorageClient> SchemaApi<S> {
    /// Creating a New Schema API Instance
    pub fn new(storage: Arc<RwLock<S>>) -> Self {
        Self { storage }
    }

    /// Creating a graph space
    ///
    /// # Parameters
    /// - `name': name of the space
    /// - `config`: space configuration
    pub fn create_space(&self, name: &str, config: SpaceConfig) -> CoreResult<()> {
        let mut space_info = SpaceInfo::new(name.to_string())
            .with_vid_type(config.vid_type)
            .with_comment(config.comment);

        let mut storage = self.storage.write();
        storage
            .create_space(&mut space_info)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        log::info!("Created graph space successfully: {}", name);
        Ok(())
    }

    /// Deletion of map space
    ///
    /// # Parameters
    /// - `name`: space name
    pub fn drop_space(&self, name: &str) -> CoreResult<()> {
        let mut storage = self.storage.write();
        let result = storage
            .drop_space(name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        if result {
            log::info!("Deleted graph space successfully: {}", name);
            Ok(())
        } else {
            Err(CoreError::NotFound(format!(
                "Graph space '{}' does not exist",
                name
            )))
        }
    }

    /// Use of map space
    ///
    /// # Parameters
    /// - `name`: space name
    ///
    /// # Return
    /// Space ID
    pub fn use_space(&self, name: &str) -> CoreResult<u64> {
        let storage = self.storage.write();
        let space_id = storage
            .get_space_id(name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        log::info!("Using graph space: {} (ID: {})", name, space_id);
        Ok(space_id)
    }

    /// List all graph spaces
    ///
    /// # Returns
    /// List of space information
    pub fn list_spaces(&self) -> CoreResult<Vec<crate::core::types::SpaceInfo>> {
        let storage = self.storage.write();
        let spaces = storage
            .list_spaces()
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        log::info!("Listed {} graph spaces", spaces.len());
        Ok(spaces)
    }

    /// Creating Tags
    ///
    /// # Parameters
    /// `space_id`: Space ID
    /// - `name`: label name
    /// - `properties`: list of property definitions
    pub fn create_tag(
        &self,
        space_id: u64,
        name: &str,
        properties: Vec<PropertyDef>,
    ) -> CoreResult<()> {
        // Get space name
        let space_name = self.get_space_name_by_id(space_id)?;

        // Conversion Attribute Definition
        let core_properties: Vec<crate::core::types::PropertyDef> =
            properties.into_iter().map(|p| p.into()).collect();

        let tag_info = TagInfo::new(name.to_string()).with_properties(core_properties);

        let mut storage = self.storage.write();
        let result = storage
            .create_tag(&space_name, &tag_info)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        log::info!(
            "Created tag successfully: {} in space {} with id {}",
            name,
            space_id,
            result
        );
        Ok(())
    }

    /// Delete Tags
    ///
    /// # Parameters
    /// - `space_id`: Space ID
    /// - `name`: tag name
    pub fn drop_tag(&self, space_id: u64, name: &str) -> CoreResult<()> {
        let space_name = self.get_space_name_by_id(space_id)?;

        let mut storage = self.storage.write();
        let result = storage
            .drop_tag(&space_name, name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        if result {
            log::info!("Deleted tag successfully: {} from space {}", name, space_id);
            Ok(())
        } else {
            Err(CoreError::NotFound(format!(
                "Tag '{}' does not exist",
                name
            )))
        }
    }

    /// Alter Tag
    ///
    /// # Parameters
    /// - `space_id`: Space ID
    /// - `tag_name`: tag name
    /// - `additions`: list of properties to add
    /// - `deletions`: list of property names to delete
    pub fn alter_tag(
        &self,
        space_id: u64,
        tag_name: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> CoreResult<()> {
        let space_name = self.get_space_name_by_id(space_id)?;

        // Convert PropertyDef to core PropertyDef
        let core_additions: Vec<crate::core::types::PropertyDef> =
            additions.into_iter().map(|p| p.into()).collect();

        let mut storage = self.storage.write();
        let result = storage
            .alter_tag(&space_name, tag_name, core_additions, deletions)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        if result {
            log::info!(
                "Altered tag successfully: {} in space {}",
                tag_name,
                space_id
            );
            Ok(())
        } else {
            Err(CoreError::NotFound(format!(
                "Tag '{}' does not exist or no changes made",
                tag_name
            )))
        }
    }

    /// Creating Edge Types
    ///
    /// # Parameters
    /// - `space_id`: space ID
    /// - `name`: name of edge type
    /// - `properties`: list of property definitions
    pub fn create_edge_type(
        &self,
        space_id: u64,
        name: &str,
        properties: Vec<PropertyDef>,
    ) -> CoreResult<()> {
        let space_name = self.get_space_name_by_id(space_id)?;

        // Conversion Attribute Definition
        let core_properties: Vec<crate::core::types::PropertyDef> =
            properties.into_iter().map(|p| p.into()).collect();

        let edge_type_info = EdgeTypeInfo::new(name.to_string()).with_properties(core_properties);

        let mut storage = self.storage.write();
        let result = storage
            .create_edge_type(&space_name, &edge_type_info)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        log::info!(
            "Created edge type successfully: {} in space {} with id {}",
            name,
            space_id,
            result
        );
        Ok(())
    }

    /// Delete Edge Type
    ///
    /// # Parameters
    /// - `space_id`: Space ID
    /// - `name`: name of edge type
    pub fn drop_edge_type(&self, space_id: u64, name: &str) -> CoreResult<()> {
        let space_name = self.get_space_name_by_id(space_id)?;

        let mut storage = self.storage.write();
        let result = storage
            .drop_edge_type(&space_name, name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        if result {
            log::info!(
                "Deleted edge type successfully: {} from space {}",
                name,
                space_id
            );
            Ok(())
        } else {
            Err(CoreError::NotFound(format!(
                "Edge type '{}' does not exist",
                name
            )))
        }
    }

    /// Alter Edge Type
    ///
    /// # Parameters
    /// - `space_id`: Space ID
    /// - `edge_type_name`: edge type name
    /// - `additions`: list of properties to add
    /// - `deletions`: list of property names to delete
    pub fn alter_edge_type(
        &self,
        space_id: u64,
        edge_type_name: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> CoreResult<()> {
        let space_name = self.get_space_name_by_id(space_id)?;

        // Convert PropertyDef to core PropertyDef
        let core_additions: Vec<crate::core::types::PropertyDef> =
            additions.into_iter().map(|p| p.into()).collect();

        let mut storage = self.storage.write();
        let result = storage
            .alter_edge_type(&space_name, edge_type_name, core_additions, deletions)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        if result {
            log::info!(
                "Altered edge type successfully: {} in space {}",
                edge_type_name,
                space_id
            );
            Ok(())
        } else {
            Err(CoreError::NotFound(format!(
                "Edge type '{}' does not exist or no changes made",
                edge_type_name
            )))
        }
    }

    /// Creating Indexes
    ///
    /// # Parameters
    /// - `space_id`: Space ID
    /// - **Name**: Index name
    /// - `target`: index target (label or edge type)
    pub fn create_index(&self, space_id: u64, name: &str, target: IndexTarget) -> CoreResult<()> {
        let space_name = self.get_space_name_by_id(space_id)?;

        // Build indexes based on target type
        let (schema_name, fields, index_type) = match target {
            IndexTarget::Tag {
                name: tag_name,
                fields,
            } => {
                // Get label information to determine field type
                let storage = self.storage.read();
                let tag_info = storage
                    .get_tag(&space_name, &tag_name)
                    .map_err(|e| CoreError::StorageError(e.to_string()))?;

                let tag_info = tag_info.ok_or_else(|| {
                    CoreError::NotFound(format!("Tag '{}' does not exist", tag_name))
                })?;

                // Building Index Fields
                let index_fields = self.build_index_fields(&fields, &tag_info.properties)?;
                (tag_name, index_fields, IndexType::TagIndex)
            }
            IndexTarget::Edge {
                name: edge_name,
                fields,
            } => {
                // Get edge type information to determine field type
                let storage = self.storage.read();
                let edge_info = storage
                    .get_edge_type(&space_name, &edge_name)
                    .map_err(|e| CoreError::StorageError(e.to_string()))?;

                let edge_info = edge_info.ok_or_else(|| {
                    CoreError::NotFound(format!("Edge type '{}' does not exist", edge_name))
                })?;

                // Building Index Fields
                let index_fields = self.build_index_fields(&fields, &edge_info.properties)?;
                (edge_name, index_fields, IndexType::EdgeIndex)
            }
        };

        // Call the corresponding creation method based on the index type
        let mut storage = self.storage.write();
        let result = match index_type {
            IndexType::TagIndex => {
                let index = Index {
                    id: 0, // Allocated by the storage layer
                    name: name.to_string(),
                    space_id,
                    schema_name,
                    fields,
                    properties: Vec::new(),
                    index_type: IndexType::TagIndex,
                    status: IndexStatus::Active,
                    is_unique: false,
                    comment: None,
                    partial_condition: None,
                };
                storage.create_tag_index(&space_name, &index)
            }
            IndexType::EdgeIndex => {
                return Err(CoreError::StorageError(
                    "edge indexes are not supported".to_string(),
                ));
            }
        }
        .map_err(|e| CoreError::StorageError(e.to_string()))?;

        if result {
            log::info!(
                "Created index successfully: {} in space {:?}",
                name,
                space_id
            );
            Ok(())
        } else {
            Err(CoreError::SchemaOperationFailed(format!(
                "Failed to create index '{}'",
                name
            )))
        }
    }

    /// Delete the index.
    ///
    /// # Parameters
    /// - `space_id`: Space ID
    /// - `name`: index name
    pub fn drop_index(&self, space_id: u64, name: &str) -> CoreResult<()> {
        let space_name = self.get_space_name_by_id(space_id)?;

        let mut storage = self.storage.write();

        // Try to delete the tag index.
        if let Ok(Some(_)) = storage.get_tag_index(&space_name, name) {
            let result = storage
                .drop_tag_index(&space_name, name)
                .map_err(|e| CoreError::StorageError(e.to_string()))?;
            if result {
                log::info!(
                    "Deleted tag index successfully: {} from space {}",
                    name,
                    space_id
                );
                return Ok(());
            }
        }

        Err(CoreError::NotFound(format!(
            "Index '{}' does not exist",
            name
        )))
    }

    /// Rebuild Index
    ///
    /// # Parameters
    /// - `space_id`: Space ID
    /// - `index_name`: index name
    pub fn rebuild_index(&self, space_id: u64, index_name: &str) -> CoreResult<()> {
        let space_name = self.get_space_name_by_id(space_id)?;

        let mut storage = self.storage.write();

        // Try to rebuild tag index
        if let Ok(Some(_)) = storage.get_tag_index(&space_name, index_name) {
            let result = storage
                .rebuild_tag_index(&space_name, index_name)
                .map_err(|e| CoreError::StorageError(e.to_string()))?;
            if result {
                log::info!(
                    "Rebuilt tag index successfully: {} in space {}",
                    index_name,
                    space_id
                );
                return Ok(());
            }
        }

        Err(CoreError::NotFound(format!(
            "Index '{}' does not exist",
            index_name
        )))
    }

    /// View the Schema
    ///
    /// # Parameters
    /// - `space_id`: Space ID
    ///
    /// # Back
    /// The “Schema” describes a string.
    pub fn describe_schema(&self, space_id: u64) -> CoreResult<String> {
        let storage = self.storage.read();

        // Obtaining spatial information
        let space_info = storage
            .get_space_by_id(space_id)
            .map_err(|e| CoreError::StorageError(e.to_string()))?
            .ok_or_else(|| CoreError::NotFound(format!("Space ID {} does not exist", space_id)))?;

        let space_name = &space_info.space_name;

        // Get all tags
        let tags = storage
            .list_tags(space_name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        // Retrieve all edge types
        let edge_types = storage
            .list_edge_types(space_name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        // Retrieve all indexes
        let tag_indexes = storage
            .list_tag_indexes(space_name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        let edge_indexes: Vec<crate::core::types::Index> = Vec::new();

        // Construct a descriptive string
        let mut description = format!("Graph space: {} (ID: {})", space_name, space_id);
        description.push_str(&format!("VID Type: {:?}", space_info.vid_type));
        if let Some(ref comment) = space_info.comment {
            description.push_str(&format!("Comment: {}", comment));
        }
        description.push('\n');

        // Tag information
        description.push_str("Tags:\n");
        if tags.is_empty() {
            description.push_str("  (none)\n");
        } else {
            for tag in &tags {
                description.push_str(&format!("  - {}\n", tag.tag_name));
                for prop in &tag.properties {
                    description.push_str(&format!(
                        "      {}: {:?}{}\n",
                        prop.name,
                        prop.data_type,
                        if prop.nullable { " (nullable)" } else { "" }
                    ));
                }
            }
        }
        description.push('\n');

        // Edge type information
        description.push_str("Edge types:\n");
        if edge_types.is_empty() {
            description.push_str("  (none)\n");
        } else {
            for edge in &edge_types {
                description.push_str(&format!("  - {}\n", edge.edge_type_name));
                for prop in &edge.properties {
                    description.push_str(&format!(
                        "      {}: {:?}{}\n",
                        prop.name,
                        prop.data_type,
                        if prop.nullable { " (nullable)" } else { "" }
                    ));
                }
            }
        }
        description.push('\n');

        // Index information
        description.push_str("Indexes:\n");
        if tag_indexes.is_empty() && edge_indexes.is_empty() {
            description.push_str("  (none)\n");
        } else {
            for idx in &tag_indexes {
                description.push_str(&format!("  - {} (tag: {})\n", idx.name, idx.schema_name));
            }
            for idx in &edge_indexes {
                description.push_str(&format!("  - {} (edge: {})\n", idx.name, idx.schema_name));
            }
        }

        log::info!("Viewing Schema: space {}", space_id);
        Ok(description)
    }
}

// Internal auxiliary methods
impl<S: StorageClient> SchemaApi<S> {
    /// Retrieve the space name based on the space ID.
    fn get_space_name_by_id(&self, space_id: u64) -> CoreResult<String> {
        let storage = self.storage.read();
        let space_info = storage
            .get_space_by_id(space_id)
            .map_err(|e| CoreError::StorageError(e.to_string()))?
            .ok_or_else(|| CoreError::NotFound(format!("Space ID {} does not exist", space_id)))?;
        Ok(space_info.space_name)
    }

    /// Construct a list of index fields
    fn build_index_fields(
        &self,
        field_names: &[String],
        properties: &[crate::core::types::PropertyDef],
    ) -> CoreResult<Vec<IndexField>> {
        let mut fields = Vec::new();

        for field_name in field_names {
            let prop = properties
                .iter()
                .find(|p| &p.name == field_name)
                .ok_or_else(|| {
                    CoreError::InvalidParameter(format!("Field '{}' does not exist", field_name))
                })?;

            // Create the corresponding Value type for the IndexField.
            let value_type = Self::datatype_to_value(&prop.data_type);

            fields.push(IndexField::new(
                field_name.clone(),
                value_type,
                prop.nullable,
            ));
        }

        Ok(fields)
    }

    /// Convert the DataType to a Value (for use with index field types).
    fn datatype_to_value(data_type: &crate::core::DataType) -> crate::core::Value {
        use crate::core::value::date_time::{DateTimeValue, DateValue, TimeValue};
        use crate::core::value::NullType;
        use crate::core::DataType;
        use crate::core::Value;

        match data_type {
            DataType::SmallInt => Value::SmallInt(0),
            DataType::Int => Value::Int(0),
            DataType::BigInt => Value::BigInt(0),
            DataType::Float => Value::Float(0.0),
            DataType::Double => Value::Double(0.0),
            DataType::String | DataType::FixedString(_) => Value::String(String::new()),
            DataType::Bool => Value::Bool(false),
            DataType::Date => Value::Date(DateValue {
                year: 1970,
                month: 1,
                day: 1,
            }),
            DataType::DateTime | DataType::Timestamp => Value::DateTime(DateTimeValue {
                year: 1970,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            }),
            DataType::Time => Value::Time(TimeValue {
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            }),
            _ => Value::Null(NullType::Null),
        }
    }
}

impl<S: StorageClient> Clone for SchemaApi<S> {
    fn clone(&self) -> Self {
        Self {
            storage: Arc::clone(&self.storage),
        }
    }
}

// Type conversion implementation
impl From<PropertyDef> for crate::core::types::PropertyDef {
    fn from(prop: PropertyDef) -> Self {
        Self {
            name: prop.name,
            data_type: prop.data_type,
            nullable: prop.nullable,
            default: prop.default_value,
            comment: prop.comment,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MockStorage;

    fn create_mock_storage() -> Arc<RwLock<MockStorage>> {
        Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ))
    }

    #[test]
    fn test_schema_api_new() {
        let storage = create_mock_storage();
        let _schema_api = SchemaApi::new(storage);
        // Creation was successful, and the test passed. Reaching this point indicates that the goal has been achieved.
    }

    #[test]
    fn test_schema_api_clone() {
        let storage = create_mock_storage();
        let schema_api = SchemaApi::new(storage);
        let _cloned = schema_api.clone();
        // The cloning was successful, and the tests have passed. Reaching this point indicates that the entire process has been a success.
    }

    #[test]
    fn test_property_def_conversion() {
        let api_prop = PropertyDef {
            name: "test".to_string(),
            data_type: crate::core::DataType::String,
            nullable: true,
            default_value: None,
            comment: Some("test comment".to_string()),
        };

        let core_prop: crate::core::types::PropertyDef = api_prop.into();
        assert_eq!(core_prop.name, "test");
        assert_eq!(core_prop.data_type, crate::core::DataType::String);
        assert!(core_prop.nullable);
        assert_eq!(core_prop.comment, Some("test comment".to_string()));
    }
}
