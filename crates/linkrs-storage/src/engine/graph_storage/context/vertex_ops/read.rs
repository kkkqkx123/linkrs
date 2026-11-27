use crate::vertex::VertexRecord;
use linkrs_core::types::{LabelId, Timestamp, VertexId};
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
        projection: &[std::sync::Arc<str>],
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        // ID-index hits are mapping-revalidated; the record itself is
        // pending-aware through `read_record`. A key with no internal id can
        // still be a staged insert of this transaction.
        let internal_id = self.persistent.data_store.with_vertex_tables(|tables| {
            let table = tables.get(&label)?;
            self.resolve_internal_id_rechecked(table, label, ExternalRef::Str(external_id), ts)
        });
        match internal_id {
            Some(internal_id) => self.read_record(label, internal_id, Some(projection), ts),
            None => self.staged_insert_record(
                label,
                &crate::vertex::IdKey::Text(external_id.to_string()),
                Some(projection),
            ),
        }
    }

    pub fn get_vertex_by_i64_projected(
        &self,
        label: LabelId,
        external_id: i64,
        projection: &[std::sync::Arc<str>],
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
        });
        match internal_id {
            Some(internal_id) => self.read_record(label, internal_id, Some(projection), ts),
            None => self.staged_insert_record(
                label,
                &crate::vertex::IdKey::Int(external_id),
                Some(projection),
            ),
        }
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

    /// Resolve integer external ids from internal handles under one
    /// vertex-table lock. Mixed labels and non-integer keys decline the
    /// lookup so callers keep the authoritative external-id path.
    pub fn external_i64_ids_by_internal_handles(
        &self,
        handles: &[Option<(u32, LabelId)>],
        ts: Timestamp,
    ) -> Option<Vec<Option<i64>>> {
        let mut label = None;
        for handle in handles {
            let Some((_, handle_label)) = *handle else {
                return None;
            };
            match label {
                None => label = Some(handle_label),
                Some(existing) if existing != handle_label => return None,
                Some(_) => {}
            }
        }
        let label = label?;
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Some(vec![None; handles.len()]);
        }
        let ids: Vec<u32> = handles
            .iter()
            .map(|handle| {
                handle
                    .map(|(internal, _)| internal)
                    .expect("handles were validated above")
            })
            .collect();
        self.persistent.data_store.with_vertex_tables(|vertex_tables| {
            let Some(table) = vertex_tables.get(&label) else {
                return Some(vec![None; handles.len()]);
            };
            let mut keys: Vec<Option<crate::vertex::IdKey>> = Vec::new();
            table.get_external_ids_raw(&ids, &mut keys);
            Some(
                keys.into_iter()
                    .zip(ids.iter())
                    .map(|(key, id)| {
                        key.or_else(|| table.get_external_id(*id, ts))
                            .and_then(|key| match key {
                                crate::vertex::IdKey::Int(value) if value >= 0 => Some(value),
                                _ => None,
                            })
                    })
                    .collect(),
            )
        })
    }

    /// Resolve many internal row ids under one vertex-table lock.
    ///
    /// Raw ids win over timestamp-valid ids so dangling edge endpoints still
    /// resolve, matching the single-id resolver's semantics.
    pub fn external_ids_by_internal_batch(
        &self,
        label: LabelId,
        internal_ids: &[u32],
        ts: Timestamp,
    ) -> Vec<Option<VertexId>> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return vec![None; internal_ids.len()];
        }
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let Some(table) = vertex_tables.get(&label) else {
                    return vec![None; internal_ids.len()];
                };
                let mut keys: Vec<Option<crate::vertex::IdKey>> = Vec::new();
                table.get_external_ids_raw(internal_ids, &mut keys);
                keys.into_iter()
                    .zip(internal_ids)
                    .map(|(key, id)| {
                        key.or_else(|| table.get_external_id(*id, ts))
                            .and_then(|key| match key {
                                crate::vertex::IdKey::Int(i) => VertexId::try_from_int64(i).ok(),
                                crate::vertex::IdKey::Text(s) => VertexId::try_from_string(&s).ok(),
                            })
                    })
                    .collect()
            })
    }
}
