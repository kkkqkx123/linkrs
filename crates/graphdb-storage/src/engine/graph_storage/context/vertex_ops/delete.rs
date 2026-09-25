use graphdb_core::types::{LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult};
use std::sync::atomic::Ordering;

use super::super::GraphStorageContext;

impl GraphStorageContext {
    pub fn delete_vertex(
        &self,
        label: LabelId,
        external_id: &str,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        // Caller-owned staging scope: the delete is buffered and applied at
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
        let internal_id = table.get_internal_id(external_id, ts);
        table
            .delete_with_scope(external_id, ts, &mut scope)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;
        self.commit_write_scope(label, &mut scope, ts)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;

        self.persistent
            .cache_manager
            .remove_cached_vertex_id(label, external_id);
        if let Some(id) = internal_id {
            self.persistent
                .cache_manager
                .remove_cached_vertex(label, id);
        }
        self.mark_vertex_modified(label);

        Ok(())
    }

    pub fn delete_vertex_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let external_id_str = external_id.to_string();
        let mut scope = crate::vertex::WriteScope::new(ts);
        let table = self
            .persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                vertex_tables.get(&label).cloned().ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })
            })?;
        let internal_id = table.get_internal_id_by_i64(external_id, ts);
        table
            .delete_by_i64_with_scope(external_id, ts, &mut scope)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;
        self.commit_write_scope(label, &mut scope, ts)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;

        self.persistent
            .cache_manager
            .remove_cached_vertex_id(label, &external_id_str);
        if let Some(id) = internal_id {
            self.persistent
                .cache_manager
                .remove_cached_vertex(label, id);
        }
        self.mark_vertex_modified(label);

        Ok(())
    }

    pub fn batch_delete_vertices(
        &self,
        label: LabelId,
        external_ids: &[&str],
        ts: Timestamp,
    ) -> StorageResult<usize> {
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
        // Resolve internal IDs before deletion: the record cache is
        // forward-compatible (cached_at_ts <= query_ts hits), so the
        // cached vertex records must be invalidated alongside the ID
        // mappings, and post-delete lookups would return None.
        let internal_ids: Vec<Option<u32>> = external_ids
            .iter()
            .map(|id| table.get_internal_id(id, ts))
            .collect();
        let staged = table.batch_delete_with_scope(external_ids, ts, &mut scope);
        let mut count = 0usize;
        for result in &staged {
            match result {
                Ok(()) => count += 1,
                Err(error) => log::warn!("batch_delete skipped vertex: {}", error),
            }
        }
        self.commit_write_scope(label, &mut scope, ts)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;
        for (external_id, internal_id) in external_ids.iter().zip(internal_ids) {
            self.persistent
                .cache_manager
                .remove_cached_vertex_id(label, external_id);
            if let Some(id) = internal_id {
                self.persistent
                    .cache_manager
                    .remove_cached_vertex(label, id);
            }
        }

        self.mark_vertex_modified(label);

        Ok(count)
    }

    pub fn batch_delete_vertices_by_i64(
        &self,
        label: LabelId,
        external_ids: &[i64],
        ts: Timestamp,
    ) -> StorageResult<usize> {
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
        // Resolve internal IDs before deletion (see batch_delete_vertices).
        let internal_ids: Vec<Option<u32>> = external_ids
            .iter()
            .map(|id| table.get_internal_id_by_i64(*id, ts))
            .collect();
        let staged = table.batch_delete_i64_with_scope(external_ids, ts, &mut scope);
        let mut count = 0usize;
        for result in &staged {
            match result {
                Ok(()) => count += 1,
                Err(error) => log::warn!("batch_delete skipped vertex: {}", error),
            }
        }
        self.commit_write_scope(label, &mut scope, ts)
            .inspect_err(|_| {
                self.rollback_write_scope(label, &mut scope, ts);
            })?;
        for (external_id, internal_id) in external_ids.iter().zip(internal_ids) {
            self.persistent
                .cache_manager
                .remove_cached_vertex_id(label, &external_id.to_string());
            if let Some(id) = internal_id {
                self.persistent
                    .cache_manager
                    .remove_cached_vertex(label, id);
            }
        }
        self.mark_vertex_modified(label);

        Ok(count)
    }
}
