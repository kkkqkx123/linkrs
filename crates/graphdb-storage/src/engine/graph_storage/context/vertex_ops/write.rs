use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};
use std::sync::atomic::Ordering;

use crate::vertex::WriteScope;

use super::super::GraphStorageContext;

impl GraphStorageContext {
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

    /// Undo rows installed by a previous [`Self::commit_write_scope`] apply.
    ///
    /// Write entries that must fail the whole request after the table apply
    /// succeeded (secondary index maintenance, mutation recording) call this
    /// so a failed request leaves no applied row behind.
    pub(crate) fn undo_applied_scope_inserts(&self, label: LabelId, global_ids: &[u32]) {
        self.persistent.data_store.with_vertex_tables(|tables| {
            if let Some(table) = tables.get(&label) {
                table.undo_applied_ids(global_ids);
            }
        });
    }
}
