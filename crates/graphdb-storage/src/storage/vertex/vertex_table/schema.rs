//! Vertex Table Schema Management
//!
//! Handles schema operations like adding, removing, and renaming properties.
//! Schema modifications invalidate the property index cache, which is rebuilt on-demand.

use crate::core::StorageResult;
use crate::storage::types::StoragePropertyDef;
use crate::storage::schema::{ChangeDetails, PropertyChange, SchemaObjectType};

use super::core::VertexTable;

impl VertexTable {
    pub fn add_property(&mut self, prop: StoragePropertyDef) -> StorageResult<()> {
        if !self.is_open {
            return Err(crate::core::StorageError::storage_not_open());
        }

        if self.columns.get_column(&prop.name).is_some() {
            return Err(crate::core::StorageError::column_already_exists(prop.name.clone()));
        }

        // Add to columns first (potentially failing operation)
        self.columns
            .add_column(prop.name.clone(), prop.data_type.clone(), prop.nullable);

        // Only modify schema if columns addition succeeded
        self.schema.properties.push(prop.clone());

        // Update cache with new property
        let idx = self.schema.properties.len() - 1;
        self.property_index_cache.insert(prop.name.clone(), idx);

        // Increment schema version on modification
        self.schema.increment_version();

        // Record schema change
        let change = PropertyChange::new(
            self.schema.schema_version,
            SchemaObjectType::Vertex,
            self.label,
            self.label_name.clone(),
            ChangeDetails::PropertyAdded {
                name: prop.name.clone(),
                data_type: prop.data_type.clone(),
                nullable: prop.nullable,
                default_value: prop.default_value.clone(),
            },
        );
        self.version_history.lock().unwrap().add_change(change);

        Ok(())
    }

    pub fn remove_property(&mut self, prop_name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(crate::core::StorageError::storage_not_open());
        }

        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == prop_name)
            .ok_or_else(|| crate::core::StorageError::column_not_found(prop_name.to_string()))?;

        // GUARD: Prevent removal of primary key property
        if index == self.schema.primary_key_index {
            return Err(crate::core::StorageError::not_supported(
                "Removing the primary key property is not supported".to_string(),
            ));
        }

        // Get property details before removal for change recording
        let removed_prop = self.schema.properties[index].clone();

        // Remove from columns first (potentially failing operation)
        self.columns.remove_column(prop_name)?;

        // Only modify schema if columns removal succeeded
        self.schema.properties.remove(index);
        if index < self.schema.primary_key_index {
            self.schema.primary_key_index -= 1;
        }

        // Rebuild cache: remove deleted property and adjust indices
        self.property_index_cache.remove(prop_name);
        for (name, idx) in &mut self.property_index_cache {
            if *idx > index {
                *idx -= 1;
            }
        }

        // Increment schema version on modification
        self.schema.increment_version();

        // Record schema change
        let change = PropertyChange::new(
            self.schema.schema_version,
            SchemaObjectType::Vertex,
            self.label,
            self.label_name.clone(),
            ChangeDetails::PropertyRemoved {
                name: removed_prop.name,
                data_type: removed_prop.data_type,
            },
        );
        self.version_history.lock().unwrap().add_change(change);

        Ok(())
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(crate::core::StorageError::storage_not_open());
        }

        if self
            .schema
            .properties
            .iter()
            .any(|prop| prop.name == new_name)
        {
            return Err(crate::core::StorageError::column_already_exists(new_name.to_string()));
        }

        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == old_name)
            .ok_or_else(|| crate::core::StorageError::column_not_found(old_name.to_string()))?;

        // Rename in columns first (potentially failing operation)
        self.columns.rename_column(old_name, new_name.to_string())?;

        // Only modify schema if columns rename succeeded
        self.schema.properties[index].name = new_name.to_string();

        // Update cache: rename key, keep index
        if let Some(idx) = self.property_index_cache.remove(old_name) {
            self.property_index_cache.insert(new_name.to_string(), idx);
        }

        // Increment schema version on modification
        self.schema.increment_version();

        // Record schema change
        let change = PropertyChange::new(
            self.schema.schema_version,
            SchemaObjectType::Vertex,
            self.label,
            self.label_name.clone(),
            ChangeDetails::PropertyRenamed {
                old_name: old_name.to_string(),
                new_name: new_name.to_string(),
            },
        );
        self.version_history.lock().unwrap().add_change(change);

        Ok(())
    }
}
