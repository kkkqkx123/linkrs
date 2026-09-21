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

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                let internal_id = table.get_internal_id(external_id, ts);
                table.delete(external_id, ts)?;
                Ok(internal_id)
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
        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                let internal_id = table.get_internal_id_by_i64(external_id, ts);
                table.delete_by_i64(external_id, ts)?;
                Ok(internal_id)
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

        let count = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                // Resolve internal IDs before deletion: the record cache is
                // forward-compatible (cached_at_ts <= query_ts hits), so the
                // cached vertex records must be invalidated alongside the ID
                // mappings, and post-delete lookups would return None.
                let internal_ids: Vec<Option<u32>> = external_ids
                    .iter()
                    .map(|id| table.get_internal_id(id, ts))
                    .collect();
                let count = table.batch_delete(external_ids, ts)?;
                Ok((count, internal_ids))
            })
            .map(|(count, internal_ids)| {
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
                count
            })?;

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

        let count = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                // Resolve internal IDs before deletion (see batch_delete_vertices).
                let internal_ids: Vec<Option<u32>> = external_ids
                    .iter()
                    .map(|id| table.get_internal_id_by_i64(*id, ts))
                    .collect();
                let count = table.batch_delete_i64(external_ids, ts)?;
                Ok((count, internal_ids))
            })
            .map(|(count, internal_ids)| {
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
                count
            })?;
        self.mark_vertex_modified(label);

        Ok(count)
    }
}
