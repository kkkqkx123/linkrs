use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};
use std::sync::atomic::Ordering;

use crate::vertex::WriteScope;

use super::super::GraphStorageContext;

impl GraphStorageContext {
    pub fn insert_vertex(
        &self,
        label: LabelId,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.insert(external_id, properties, ts)
            })?;

        self.persistent
            .cache_manager
            .cache_vertex_id(label, external_id, internal_id, ts);
        self.mark_vertex_modified(label);
        self.observe_vertex_id_string(label);

        Ok(internal_id)
    }

    pub fn insert_vertex_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        // Single rejection point for negative ids lives in
        // VertexId::try_from_int64; the table layer re-checks as backstop.
        VertexId::try_from_int64(external_id)?;
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.insert_by_i64(external_id, properties, ts)
            })?;

        self.persistent.cache_manager.cache_vertex_id(
            label,
            &external_id.to_string(),
            internal_id,
            ts,
        );
        self.mark_vertex_modified(label);
        self.observe_vertex_id_i64(label, external_id);

        Ok(internal_id)
    }

    /// Scoped vertex insert staging the caller-owned row.
    ///
    /// Buffers the validated row in the scope without touching global
    /// state; the caller applies it through [`Self::commit_write_scope`].
    /// The scope travels by mutable borrow and is destroyed by the commit /
    /// rollback hooks or by crash drop.
    pub fn insert_vertex_with_scope(
        &self,
        label: LabelId,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.insert_with_scope(external_id, properties, ts, scope)
            })?;
        Ok(())
    }

    /// Integer-keyed scoped insert staging. Same contract as
    /// [`Self::insert_vertex_with_scope`].
    pub fn insert_vertex_by_i64_with_scope(
        &self,
        label: LabelId,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        VertexId::try_from_int64(external_id)?;
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.insert_by_i64_with_scope(external_id, properties, ts, scope)
            })?;
        Ok(())
    }

    /// Commit hook for one label's staged rows: applies them to the table,
    /// then drops the label's staging records. Called before the timestamp
    /// commit; WAL durability stays the commit point. Returns the staged
    /// key to allocated global id mapping for the label.
    pub fn commit_write_scope(
        &self,
        label: LabelId,
        scope: &mut WriteScope,
        ts: Timestamp,
    ) -> StorageResult<Vec<(crate::vertex::IdKey, u32)>> {
        self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .get(&label)
                .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))
                .and_then(|table| table.commit_write_scope(scope, ts))
        })
    }

    /// Rollback hook for one label's staged rows. Discards the scope records
    /// without touching global state. Called before the timestamp abort and
    /// from the undo failure branches.
    pub fn rollback_write_scope(&self, label: LabelId, scope: &mut WriteScope, ts: Timestamp) {
        self.persistent.data_store.with_vertex_tables(|tables| {
            if let Some(table) = tables.get(&label) {
                table.rollback_write_scope(scope, ts);
            }
        });
    }
}
