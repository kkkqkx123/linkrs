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
            .with_vertex_tables_mut(|vertex_tables| {
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
            .with_vertex_tables_mut(|vertex_tables| {
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

    /// Scoped vertex insert threading the caller-owned write scope.
    ///
    /// Forwards the scope through the shard table without retaining it; the
    /// caller destroys the scope on commit, rollback, or crash.
    pub fn insert_vertex_with_scope(
        &self,
        label: LabelId,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<u32> {
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
                table.insert_with_scope(external_id, properties, ts, scope)
            })?;
        debug_assert!(matches!(
            self.lookup_pk_scoped(label, external_id, ts, scope),
            crate::vertex::PkLookup::Visible(id) if id == internal_id
        ));
        self.persistent
            .cache_manager
            .cache_vertex_id(label, external_id, internal_id, ts);
        self.mark_vertex_modified(label);
        self.observe_vertex_id_string(label);
        Ok(internal_id)
    }

    /// Integer-keyed scoped insert. Same forwarding contract as
    /// [`Self::insert_vertex_with_scope`].
    pub fn insert_vertex_by_i64_with_scope(
        &self,
        label: LabelId,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<u32> {
        VertexId::try_from_int64(external_id)?;
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
                table.insert_by_i64_with_scope(external_id, properties, ts, scope)
            })?;
        debug_assert!(matches!(
            self.lookup_pk_by_i64_scoped(label, external_id, ts, scope),
            crate::vertex::PkLookup::Visible(id) if id == internal_id
        ));
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

    /// Commit hook for one label's staged bindings. Called before the
    /// timestamp commit; WAL durability stays the commit point.
    pub fn commit_write_scope(&self, label: LabelId, scope: &mut WriteScope) -> usize {
        self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .get(&label)
                .map(|table| table.commit_write_scope(scope))
                .unwrap_or(0)
        })
    }

    /// Rollback hook for one label's staged bindings. Discards the scope
    /// records and logically deletes already applied rows with the existing
    /// undo semantics. Called before the timestamp abort and from the undo
    /// failure branches.
    pub fn rollback_write_scope(&self, label: LabelId, scope: &mut WriteScope, ts: Timestamp) {
        self.persistent.data_store.with_vertex_tables(|tables| {
            if let Some(table) = tables.get(&label) {
                table.rollback_write_scope(scope, ts);
            }
        });
    }
}
