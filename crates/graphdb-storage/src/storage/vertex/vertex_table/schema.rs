//! Vertex Table Schema Management
//!
//! Handles schema operations like adding, removing, and renaming properties.
//! Schema modifications invalidate the property index cache, which is rebuilt on-demand.

use crate::core::StorageResult;
use crate::storage::types::StoragePropertyDef;
use crate::storage::schema::{ChangeDetails, PropertyChange, SchemaObjectType};

use super::core::VertexTable;

impl VertexTable {
    /// Record a schema change event
    ///
    /// Handles the common pattern of:
    /// 1. Computing next version number from history
    /// 2. Creating a PropertyChange event
    /// 3. Recording it in the version history
    fn record_schema_change(&mut self, details: ChangeDetails) -> StorageResult<()> {
        // Get the next version number from history
        let mut history_guard = self.version_history
            .lock()
            .map_err(|_| crate::core::StorageError::db_error("Failed to lock version_history"))?;

        let next_version = history_guard.latest_version() + 1;

        let change = PropertyChange::new(
            next_version,
            SchemaObjectType::Vertex,
            self.label,
            self.label_name.clone(),
            details,
        );

        history_guard.add_change(change);

        Ok(())
    }

    /// Rebuild schema change record during WAL recovery
    ///
    /// This is used during recovery when the column already exists (from SchemaManager),
    /// but we need to update version_history to reflect the schema operation in the WAL.
    /// Does NOT add the column (it already exists), but DOES record the change.
    pub fn rebuild_schema_change_from_redo(&mut self, details: ChangeDetails) -> StorageResult<()> {
        self.record_schema_change(details)
    }

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

        self.record_schema_change(ChangeDetails::PropertyAdded {
            name: prop.name.clone(),
            data_type: prop.data_type.clone(),
            nullable: prop.nullable,
            default_value: prop.default_value.clone(),
        })?;

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
        for idx in self.property_index_cache.values_mut() {
            if *idx > index {
                *idx -= 1;
            }
        }

        self.record_schema_change(ChangeDetails::PropertyRemoved {
            name: removed_prop.name,
            data_type: removed_prop.data_type,
        })?;

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

        self.record_schema_change(ChangeDetails::PropertyRenamed {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        })?;

        Ok(())
    }
}
