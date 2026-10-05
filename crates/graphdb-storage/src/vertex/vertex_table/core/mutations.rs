//! Property update/delete paths and their MVCC undo operations.

use std::sync::atomic::Ordering;

use super::VertexTable;
use crate::vertex::{primary_key_mirror_value, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

impl VertexTable {
    /// Read-only validation and normalization for one property update:
    /// primary-key mirror rejection, column existence and type checks.
    /// Staging paths call this before buffering so an invalid column fails
    /// with no global write.
    pub fn prepare_update(&self, col_name: &str, value: &Value) -> StorageResult<Value> {
        self.columns.check_column_available(col_name)?;
        if self
            .schema
            .properties
            .get(self.schema.primary_key_index)
            .is_some_and(|pk| &*pk.name == col_name)
        {
            return Err(StorageError::invalid_operation(format!(
                "Primary key column '{}' mirrors the vertex id and cannot be updated; delete and re-insert the vertex instead",
                col_name
            )));
        }

        // Use cached index lookup
        let prop_idx = self
            .property_index_cache
            .get(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;
        let prop_def = &self.schema.properties[*prop_idx];

        if value.data_type() != prop_def.data_type {
            value.try_cast_to(&prop_def.data_type)
        } else {
            Ok(value.clone())
        }
    }

    /// Validate and normalize one property update value, including the
    /// primary-key mirror rule: restating the mirror is a no-op value,
    /// while a divergent value is rejected. Shared by the apply path and
    /// the commit pre-validation so both accept the same updates.
    pub fn validate_update_value(
        &self,
        internal_id: u32,
        col_name: &str,
        value: &Value,
    ) -> StorageResult<Value> {
        // The primary key column mirrors the row's own key: restating the
        // mirror changes nothing and is a no-op (a delete-then-insert folded
        // into a whole-row update always carries it), while a divergent
        // value would fork the key from its mirror and is rejected.
        if let Some(pk_def) = self.schema.properties.get(self.schema.primary_key_index) {
            if &*pk_def.name == col_name {
                let key = self
                    .id_indexer
                    .get_key(internal_id)
                    .ok_or_else(StorageError::vertex_not_found)?;
                let mirror = primary_key_mirror_value(&pk_def.data_type, &key)?
                    .try_cast_to(&pk_def.data_type)?;
                let provided = value.try_cast_to(&pk_def.data_type)?;
                if provided != mirror {
                    return Err(StorageError::invalid_operation(format!(
                        "Primary key column '{}' must mirror the vertex id: got {:?}, expected {:?}",
                        col_name, provided, mirror
                    )));
                }
                return Ok(provided);
            }
        }
        self.prepare_update(col_name, value)
    }

    pub fn update_property(
        &self,
        internal_id: u32,
        col_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        // Hold the identity read guard across the column write: the latch
        // order allows identity before segments, and holding it blocks a
        // concurrent delete (which needs the write latch) from interleaving
        // between the liveness check and the versioned write. The probe below
        // rechecks liveness inside the segment latch through the already-held
        // map, so no new latch is acquired there.
        let stamps = self.timestamps.read();
        if !stamps.is_valid(internal_id, ts) {
            return Err(StorageError::vertex_not_found());
        }

        let converted_value = self.validate_update_value(internal_id, col_name, value)?;
        if self.is_pk_column(col_name) {
            return Ok(());
        }

        self.columns.set_property_versioned_checked(
            internal_id as usize,
            col_name,
            Some(&converted_value),
            ts,
            || stamps.is_valid(internal_id, ts),
        )?;
        Ok(())
    }

    /// Delete one column of one row by writing a null tombstone version.
    ///
    /// Primary-key mirror columns never participate in row replacement and
    /// are rejected here as misuse.
    pub fn delete_property(
        &self,
        internal_id: u32,
        col_name: &str,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        if self.is_pk_column(col_name) {
            return Err(StorageError::invalid_operation(format!(
                "Primary key column '{}' cannot be deleted by row replacement",
                col_name
            )));
        }
        let stamps = self.timestamps.read();
        if !stamps.is_valid(internal_id, ts) {
            return Err(StorageError::vertex_not_found());
        }
        self.columns.set_property_versioned_checked(
            internal_id as usize,
            col_name,
            None,
            ts,
            || stamps.is_valid(internal_id, ts),
        )?;
        Ok(())
    }

    /// Apply one delete by internal id: timestamp tombstone under the
    /// identity latch, then the row dirty mark on the column segments.
    pub fn apply_delete(&self, internal_id: u32, ts: Timestamp) -> StorageResult<()> {
        if !self.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        {
            let mut stamps = self.timestamps.write();
            if !stamps.is_valid(internal_id, ts) {
                return Err(StorageError::vertex_not_found());
            }
            stamps.remove(internal_id, ts);
        }
        self.mark_row_dirty(internal_id as usize);
        Ok(())
    }

    pub fn delete_by_internal_id(&self, internal_id: u32, ts: Timestamp) -> StorageResult<()> {
        self.apply_delete(internal_id, ts)
    }

    pub fn revert_delete(&self, internal_id: u32) {
        self.timestamps.write().revert_delete(internal_id);
        self.mark_row_dirty(internal_id as usize);
    }

    /// Backdate one row's creation stamps for offline redistribution.
    /// Row and column layers move together so snapshot reads below the
    /// rebuild timestamp keep serving the migrated values.
    pub fn backdate_row_for_reshard(&self, internal_id: u32, create_ts: Timestamp) {
        self.timestamps.write().insert(internal_id, create_ts);
        self.columns.backdate_row(internal_id as usize, create_ts);
    }

    pub fn undo_update(
        &self,
        internal_id: u32,
        col_name: &str,
        ts: Timestamp,
    ) -> StorageResult<()> {
        self.columns
            .undo_last_versioned_write(internal_id as usize, col_name, ts)
    }
}
