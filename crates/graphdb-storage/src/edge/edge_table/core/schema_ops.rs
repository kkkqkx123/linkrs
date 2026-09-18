//! Schema evolution and property column maintenance.

use super::EdgeStore;
use crate::schema::{ChangeDetails, PropertyChange, SchemaObjectType};
use graphdb_core::{DataType, StorageError, StorageResult};

impl EdgeStore {
    /// Record a schema change event
    ///
    /// Handles the common pattern of:
    /// 1. Computing next version number from history
    /// 2. Creating a PropertyChange event
    /// 3. Recording it in the version history
    pub(crate) fn record_schema_change(&mut self, details: ChangeDetails) -> StorageResult<()> {
        let mut history_guard = self
            .version_history
            .lock()
            .map_err(|_| StorageError::db_error("Failed to lock version_history"))?;

        let next_version = history_guard.latest_version() + 1;
        self.schema.schema_version = next_version;

        let change = PropertyChange::new(
            next_version,
            SchemaObjectType::Edge,
            self.label,
            self.label_name.clone(),
            details,
        );

        history_guard.add_change(change);

        Ok(())
    }

    pub fn add_property(
        &mut self,
        name: String,
        data_type: DataType,
        nullable: bool,
    ) -> StorageResult<()> {
        // Single code path: the immediate add is prepare + fill + publish of
        // the staged state machine, so no second column-construction
        // implementation exists.
        self.prepare_add_property(name.clone(), data_type.clone(), nullable, None)?;
        if let Err(error) = self.fill_pending_add_property() {
            let _ = self.abort_pending_add_property();
            return Err(error);
        }
        self.publish_pending_add_property()
    }

    /// Select and apply encodings for every property column.
    ///
    /// Explicit maintenance operation: hot columns stay unencoded between
    /// runs by design so everyday writes never pay re-encoding. Encodings
    /// change the physical representation without changing logical values,
    /// and the encoding choice is part of the checkpoint payload, so this
    /// marks properties dirty to carry the choice to the next checkpoint.
    /// Returns the number of columns that received an encoding.
    pub fn encode_property_columns(&mut self) -> usize {
        let encoded = self.properties.auto_encode_properties();
        if encoded > 0 {
            self.mark_properties_dirty();
        }
        encoded
    }

    /// Recompute persisted per-column statistics from current contents.
    pub fn refresh_property_stats(&mut self) {
        self.properties.refresh_column_stats();
    }

    /// Rebuild schema change record during WAL recovery
    ///
    /// This is used during recovery when the column already exists (from SchemaManager),
    /// but we need to update version_history to reflect the schema operation in the WAL.
    /// Does NOT add the property (it already exists), but DOES record the change.
    pub fn rebuild_schema_change_from_redo(&mut self, details: ChangeDetails) -> StorageResult<()> {
        self.record_schema_change(details)
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        // Single code path: the immediate drop is prepare + publish of the
        // staged state machine, so no second column-removal implementation
        // exists. Publish restores from its snapshot when history recording
        // fails; other failures leave storage untouched.
        let owned = name.to_string();
        self.prepare_drop_property(&owned)?;
        if let Err(error) = self.publish_pending_drop_property() {
            let _ = self.abort_pending_drop_property();
            return Err(error);
        }
        Ok(())
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self
            .schema
            .properties
            .iter()
            .any(|prop| prop.name == new_name)
        {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }

        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == old_name)
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;

        // Rename in properties first (potentially failing operation)
        self.properties.rename_property(old_name, new_name)?;
        // Only modify schema if properties rename succeeded
        self.schema.properties[index].name = new_name.to_string();
        // Update cache: rename key, keep index
        if let Some(idx) = self.property_index_cache.remove(old_name) {
            self.property_index_cache.insert(new_name.to_string(), idx);
        }

        if let Err(error) = self.record_schema_change(ChangeDetails::PropertyRenamed {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        }) {
            // History is the last step: rename everything back so no
            // half-renamed state survives a history failure.
            let _ = self.properties.rename_property(new_name, old_name);
            self.schema.properties[index].name = old_name.to_string();
            if let Some(idx) = self.property_index_cache.remove(new_name) {
                self.property_index_cache.insert(old_name.to_string(), idx);
            }
            return Err(error);
        }
        self.mark_properties_dirty();

        Ok(())
    }
}
