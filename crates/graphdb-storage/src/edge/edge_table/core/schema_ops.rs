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
        // Single code path: the immediate rename is prepare + publish of the
        // staged state machine, so no second column-rename implementation
        // exists. Publish restores from its snapshot when history recording
        // fails; other failures leave storage untouched.
        let old = old_name.to_string();
        let new = new_name.to_string();
        self.prepare_rename_property(&old, &new)?;
        if let Err(error) = self.publish_pending_rename_property() {
            let _ = self.abort_pending_rename_property();
            return Err(error);
        }
        Ok(())
    }

    /// Table-level property fallback rewrites since creation.
    ///
    /// Counts checkpoints where property dirt carried no group trace and every
    /// owner rewrote as insurance. Flat under normal load; growth points at a
    /// missing write-time mark.
    pub fn property_fallback_rewrites(&self) -> u64 {
        self.property_fallback_rewrites
    }
}
