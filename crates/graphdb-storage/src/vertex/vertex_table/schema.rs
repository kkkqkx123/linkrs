//! Vertex Table Schema Management
//!
//! Handles schema operations like adding, removing, and renaming properties.
//! Schema modifications invalidate the property index cache, which is rebuilt on-demand.

use crate::schema::{ChangeDetails, PropertyChange, SchemaObjectType};
use graphdb_core::StorageResult;

use super::core::VertexTable;

impl VertexTable {
    /// Record a schema change event
    ///
    /// Handles the common pattern of:
    /// 1. Computing next version number from history
    /// 2. Creating a PropertyChange event
    /// 3. Recording it in the version history
    pub(super) fn record_schema_change(&mut self, details: ChangeDetails) -> StorageResult<()> {
        let mut history_guard = self
            .version_history
            .lock()
            .map_err(|_| graphdb_core::StorageError::db_error("Failed to lock version_history"))?;

        let next_version = history_guard.latest_version() + 1;
        self.schema.schema_version = next_version;

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
}
