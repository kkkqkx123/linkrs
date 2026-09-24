use crate::vertex::VertexRecord;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use std::sync::atomic::Ordering;

use super::super::GraphStorageContext;
use super::resolve::ExternalRef;

impl GraphStorageContext {
    /// Pre-allocate capacity for `additional` more vertices in the given label's table.
    /// Call before batch inserts to avoid repeated hash rehashing.
    pub fn reserve_vertex_capacity(&self, label: LabelId, additional: usize) {
        let _ = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                if let Some(table) = vertex_tables.get(&label) {
                    table.reserve_id_capacity(additional);
                }
                Ok(())
            });
    }

    pub fn get_vertex(
        &self,
        label: LabelId,
        external_id: &str,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }
        self.get_full_record(label, ExternalRef::Str(external_id), ts)
    }

    /// Fetch a vertex restricted to the given property projection, skipping
    /// the full-record cache so partial results never replace cached vertices.
    ///
    /// Pending-aware through [`Self::resolve_projected_on_table`].
    pub fn get_vertex_projected(
        &self,
        label: LabelId,
        external_id: &str,
        projection: &[String],
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        // ID-index hits are mapping-revalidated; the record itself is
        // pending-aware through `read_record`.
        let internal_id = self.persistent.data_store.with_vertex_tables(|tables| {
            let table = tables.get(&label)?;
            self.resolve_internal_id_rechecked(table, label, ExternalRef::Str(external_id), ts)
        })?;

        self.read_record(label, internal_id, Some(projection), ts)
    }

    pub fn get_vertex_by_i64_projected(
        &self,
        label: LabelId,
        external_id: i64,
        projection: &[String],
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        // ID-index hits are mapping-revalidated; the record itself is
        // pending-aware through `read_record`.
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        let internal_id = self.persistent.data_store.with_vertex_tables(|tables| {
            let table = tables.get(&label)?;
            self.resolve_internal_id_rechecked(table, label, ExternalRef::I64(external_id), ts)
        })?;

        self.read_record(label, internal_id, Some(projection), ts)
    }

    pub fn get_vertex_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        self.get_full_record(label, ExternalRef::I64(external_id), ts)
    }

    pub fn get_vertex_by_internal_id(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        self.get_full_record_by_internal_id(label, internal_id, ts)
    }

    pub fn get_external_vertex_id(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<VertexId> {
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let table = vertex_tables.get(&label)?;
                let key = table.get_external_id(internal_id, ts)?;
                match key {
                    crate::vertex::IdKey::Int(i) => VertexId::try_from_int64(i).ok(),
                    crate::vertex::IdKey::Text(s) => VertexId::try_from_string(&s).ok(),
                }
            })
    }

    pub fn get_external_id_by_internal_id(
        &self,
        label: LabelId,
        internal_id: u32,
    ) -> Option<VertexId> {
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let table = vertex_tables.get(&label)?;
                let key = table.get_external_id_raw(internal_id)?;
                match key {
                    crate::vertex::IdKey::Int(i) => VertexId::try_from_int64(i).ok(),
                    crate::vertex::IdKey::Text(s) => VertexId::try_from_string(&s).ok(),
                }
            })
    }

    /// Scoped primary-key probe: the caller's buffer wins, otherwise the
    /// global committed area. Outside scopes never observe the buffer, so
    /// uncommitted keys stay invisible off-scope while the owning scope reads
    /// its own writes.
    pub fn lookup_pk_scoped(
        &self,
        label: LabelId,
        external_id: &str,
        ts: Timestamp,
        scope: &crate::vertex::WriteScope,
    ) -> crate::vertex::PkLookup {
        self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .get(&label)
                .map(|table| table.lookup_pk_with_scope(external_id, ts, scope))
                .unwrap_or(crate::vertex::PkLookup::Missing)
        })
    }

    /// Integer-keyed scoped probe. Same contract as [`Self::lookup_pk_scoped`].
    pub fn lookup_pk_by_i64_scoped(
        &self,
        label: LabelId,
        external_id: i64,
        ts: Timestamp,
        scope: &crate::vertex::WriteScope,
    ) -> crate::vertex::PkLookup {
        self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .get(&label)
                .map(|table| table.lookup_pk_by_i64_with_scope(external_id, ts, scope))
                .unwrap_or(crate::vertex::PkLookup::Missing)
        })
    }
}
