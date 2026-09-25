use graphdb_core::types::{LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};
use std::sync::atomic::Ordering;

use super::super::GraphStorageContext;

impl GraphStorageContext {
    pub fn update_vertex_property(
        &self,
        label: LabelId,
        external_id: &str,
        property_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        // Caller-owned staging scope: the update is buffered and applied at
        // the commit hook inside this call, so a failed request leaves no
        // global write behind. The table handle is cloned out of the catalog
        // first so staging and commit never nest catalog locks.
        let mut scope = crate::vertex::WriteScope::new(ts);
        let table = self
            .persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                vertex_tables.get(&label).cloned().ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })
            })?;
        let internal_id = table
            .get_internal_id(external_id, ts)
            .ok_or(StorageError::vertex_not_found())?;
        table
            .update_property_with_scope(internal_id, property_name, value, ts, &mut scope)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;
        self.commit_write_scope(label, &mut scope, ts)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;

        self.persistent
            .cache_manager
            .remove_cached_vertex(label, internal_id);
        self.mark_vertex_modified(label);

        Ok(())
    }

    pub fn update_vertex_property_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        property_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let mut scope = crate::vertex::WriteScope::new(ts);
        let table = self
            .persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                vertex_tables.get(&label).cloned().ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })
            })?;
        let internal_id = table
            .get_internal_id_by_i64(external_id, ts)
            .ok_or(StorageError::vertex_not_found())?;
        table
            .update_property_with_scope(internal_id, property_name, value, ts, &mut scope)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;
        self.commit_write_scope(label, &mut scope, ts)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;

        self.persistent
            .cache_manager
            .remove_cached_vertex(label, internal_id);
        self.mark_vertex_modified(label);

        Ok(())
    }
}
