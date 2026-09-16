use crate::mvcc_visibility::PendingGate;
use crate::vertex::{ShardedVertexTable, VertexRecord};
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};
use std::sync::atomic::Ordering;

use super::GraphStorageContext;

/// External vertex reference for point lookups.
#[derive(Clone, Copy)]
enum ExternalRef<'a> {
    Str(&'a str),
    I64(i64),
}

impl<'a> ExternalRef<'a> {
    fn raw_internal_id(&self, table: &ShardedVertexTable) -> Option<u32> {
        match *self {
            ExternalRef::Str(id) => table.get_internal_id_raw(id),
            ExternalRef::I64(id) => table.get_internal_id_by_i64_raw(id),
        }
    }

    fn versioned_internal_id(&self, table: &ShardedVertexTable, ts: Timestamp) -> Option<u32> {
        match *self {
            ExternalRef::Str(id) => table.get_internal_id(id, ts),
            ExternalRef::I64(id) => table.get_internal_id_by_i64(id, ts),
        }
    }

    fn cache_key(&self) -> String {
        match *self {
            ExternalRef::Str(id) => id.to_string(),
            ExternalRef::I64(id) => id.to_string(),
        }
    }
}

impl GraphStorageContext {
    pub(crate) fn pending_gate(&self) -> PendingGate<'_> {
        let own_write = self
            .operation_context
            .as_ref()
            .and_then(|context| context.write_timestamp);
        PendingGate::new(&self.persistent.version_manager, own_write)
    }

    /// Owned gate inputs for closures that mutate their owner while
    /// filtering: the caller clones the manager handle up front and builds
    /// the gate inside the closure so no borrow of `self` is retained.
    pub(crate) fn gate_inputs(
        &self,
    ) -> (
        std::sync::Arc<graphdb_transaction::VersionManager>,
        Option<Timestamp>,
    ) {
        let own_write = self
            .operation_context
            .as_ref()
            .and_then(|context| context.write_timestamp);
        (self.persistent.version_manager.clone(), own_write)
    }

    /// Pending-aware point-record resolution on one table, lock-free inside.
    ///
    /// Reads through the plain predicate, then rechecks the surviving row
    /// stamps through the slot-state gate: a creation stamp owned by a
    /// foreign uncommitted transaction hides the row, a foreign pending
    /// deletion is ignored by re-reading below it, and a column value whose
    /// covering version stamp is foreign-pending falls back to `stamp - 1`.
    /// Covering stamps are monotone decreasing across retries, so the loop
    /// terminates. Returns the record with the fences describing the version
    /// actually read (creation stamp, per-column covering stamps, read ts).
    /// Scan paths reuse this funnel per row (see `scan_vertices`).
    pub(crate) fn resolve_on_table(
        table: &ShardedVertexTable,
        internal_id: u32,
        ts: Timestamp,
        gate: &PendingGate<'_>,
    ) -> Option<(VertexRecord, Timestamp, Vec<Timestamp>, Timestamp)> {
        let mut cur = ts;
        loop {
            let record = table.get_by_internal_id(internal_id, cur);
            let survival = table.row_timestamps(internal_id);
            match (record, survival) {
                (Some(record), Some((create_ts, delete_ts))) => {
                    if !gate.is_row_visible(cur, create_ts, delete_ts) {
                        return None;
                    }
                    let starts = table.row_picked_starts(internal_id, cur);
                    match starts
                        .iter()
                        .filter(|stamp| gate.is_foreign_pending(cur, **stamp))
                        .min()
                    {
                        Some(0) | None => return Some((record, create_ts, starts, cur)),
                        Some(stamp) => {
                            cur = stamp.saturating_sub(1);
                            continue;
                        }
                    }
                }
                (Some(_), None) => return None,
                (None, Some((create_ts, delete_ts))) => {
                    if gate.is_create_visible(cur, create_ts) {
                        if let Some(delete_ts) = delete_ts {
                            if delete_ts <= cur
                                && gate.is_foreign_pending(cur, delete_ts)
                                && delete_ts > 0
                            {
                                cur = delete_ts - 1;
                                continue;
                            }
                        }
                    }
                    return None;
                }
                (None, None) => return None,
            }
        }
    }

    /// Pending-aware projected read on one table.
    ///
    /// Same gate fallback as [`Self::resolve_on_table`] but decodes only the
    /// requested projection, so projected reads never consult or populate the
    /// full-record cache and never observe foreign uncommitted writes.
    pub(crate) fn resolve_projected_on_table(
        table: &ShardedVertexTable,
        internal_id: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
        gate: &PendingGate<'_>,
    ) -> Option<VertexRecord> {
        let mut cur = ts;
        loop {
            let record = table.get_projected_by_internal_id(internal_id, cur, projection);
            let survival = table.row_timestamps(internal_id);
            match (record, survival) {
                (Some(_), Some((create_ts, delete_ts))) => {
                    if !gate.is_row_visible(cur, create_ts, delete_ts) {
                        return None;
                    }
                    let starts = table.row_picked_starts(internal_id, cur);
                    match starts
                        .iter()
                        .filter(|stamp| gate.is_foreign_pending(cur, **stamp))
                        .min()
                    {
                        Some(0) | None => {
                            return table.get_projected_by_internal_id(internal_id, cur, projection)
                        }
                        Some(stamp) => {
                            cur = stamp.saturating_sub(1);
                            continue;
                        }
                    }
                }
                (Some(_), None) => return None,
                (None, Some((create_ts, delete_ts))) => {
                    if gate.is_create_visible(cur, create_ts) {
                        if let Some(delete_ts) = delete_ts {
                            if delete_ts <= cur
                                && gate.is_foreign_pending(cur, delete_ts)
                                && delete_ts > 0
                            {
                                cur = delete_ts - 1;
                                continue;
                            }
                        }
                    }
                    return None;
                }
                (None, None) => return None,
            }
        }
    }

    /// Resolve an external id with ID-index revalidation.
    ///
    /// A cached mapping is accepted only when the table still maps the
    /// external id to the same internal id (covers GC-remap and
    /// delete/recreate races); otherwise the version-aware lookup runs and
    /// reseeds the cache. Cache entries are owned clones, so the cache lock
    /// is never held while taking a table lock — no lock-order inversion
    /// (verified: no cache path reaches back into table locks; write paths
    /// invalidate after releasing table locks, i.e. the same table→cache
    /// order used here).
    fn resolve_internal_id_rechecked(
        &self,
        table: &ShardedVertexTable,
        label: LabelId,
        external: ExternalRef<'_>,
        ts: Timestamp,
    ) -> Option<u32> {
        if let Some(cached) =
            self.persistent
                .cache_manager
                .get_cached_vertex_id(label, &external.cache_key(), ts)
        {
            if external.raw_internal_id(table) == Some(cached) {
                return Some(cached);
            }
        }
        let id = external.versioned_internal_id(table, ts)?;
        self.persistent
            .cache_manager
            .cache_vertex_id(label, &external.cache_key(), id, ts);
        Some(id)
    }

    /// Revalidate a record-cache hit against live storage fences.
    ///
    /// Accepts the cached record only when the row is still alive at `ts`,
    /// its creation stamp matches the seed fence, and the per-column
    /// covering stamps match (no property write landed between seed and
    /// query). Anything else is a miss.
    fn rechecked_cached_record(
        &self,
        table: &ShardedVertexTable,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let cached = self
            .persistent
            .cache_manager
            .get_cached_vertex(label, internal_id, ts)?;
        let (create_ts, delete_ts) = table.row_timestamps(internal_id)?;
        if create_ts != cached.create_ts {
            return None;
        }
        if create_ts > ts || delete_ts.is_some_and(|end| ts >= end) {
            return None;
        }
        if table.row_picked_starts(internal_id, ts) != cached.column_starts {
            return None;
        }
        Some(VertexRecord {
            internal_id: cached.internal_id,
            vid: cached
                .external_id
                .parse::<i64>()
                .map(graphdb_core::types::VertexId::from_int64)
                .unwrap_or_else(|_| {
                    graphdb_core::types::VertexId::from_string(&cached.external_id)
                }),
            properties: cached.properties,
        })
    }

    /// Full point lookup by external id: revalidated ID resolve, revalidated
    /// record hit, pending-aware miss read with conditional seeding.
    ///
    /// The whole path runs in one catalog scope; the seed publishes the
    /// record reread together with its fences only when that version is
    /// still current (live, same creation stamp, no newer column version),
    /// so a commit landing between read and seed turns into a skipped seed
    /// instead of a poisoned entry.
    fn get_full_record(
        &self,
        label: LabelId,
        external: ExternalRef<'_>,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let gate = self.pending_gate();
        self.persistent.data_store.with_vertex_tables(|tables| {
            let table = tables.get(&label)?;
            let internal_id = self.resolve_internal_id_rechecked(table, label, external, ts)?;
            if self.record_cache_eligible(ts) {
                if let Some(hit) = self.rechecked_cached_record(table, label, internal_id, ts) {
                    return Some(hit);
                }
            }
            let (record, create_ts, starts, read_ts) =
                Self::resolve_on_table(table, internal_id, ts, &gate)?;
            if self.record_cache_eligible(ts) {
                let (live_create, live_delete) = table.row_timestamps(internal_id)?;
                // Latest-observed fence probe: versions newer than both the
                // read stamp and the published frontier (committed or foreign
                // pending) veto the seed. Never a bare maximum sentinel.
                let latest = ts.max(self.persistent.version_manager.read_timestamp());
                if live_create == create_ts
                    && live_delete.is_none()
                    && table.row_picked_starts(internal_id, latest) == starts
                {
                    self.persistent.cache_manager.cache_vertex(
                        label,
                        internal_id,
                        external.cache_key(),
                        record.properties.clone(),
                        read_ts,
                        create_ts,
                        starts,
                    );
                }
            }
            Some(record)
        })
    }

    /// Full point lookup by internal id, with the same hit revalidation and
    /// conditional seeding as [`Self::get_full_record`].
    fn get_full_record_by_internal_id(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let gate = self.pending_gate();
        self.persistent.data_store.with_vertex_tables(|tables| {
            let table = tables.get(&label)?;
            if self.record_cache_eligible(ts) {
                if let Some(hit) = self.rechecked_cached_record(table, label, internal_id, ts) {
                    return Some(hit);
                }
            }
            let (record, create_ts, starts, read_ts) =
                Self::resolve_on_table(table, internal_id, ts, &gate)?;
            if self.record_cache_eligible(ts) {
                let (live_create, live_delete) = table.row_timestamps(internal_id)?;
                // Latest-observed fence probe: versions newer than both the
                // read stamp and the published frontier (committed or foreign
                // pending) veto the seed. Never a bare maximum sentinel.
                let latest = ts.max(self.persistent.version_manager.read_timestamp());
                if live_create == create_ts
                    && live_delete.is_none()
                    && table.row_picked_starts(internal_id, latest) == starts
                {
                    let external_id = table
                        .get_external_id(internal_id, ts)
                        .map(|key| key.to_string())
                        .unwrap_or_default();
                    if !external_id.is_empty() {
                        self.persistent.cache_manager.cache_vertex_id(
                            label,
                            &external_id,
                            internal_id,
                            ts,
                        );
                        self.persistent.cache_manager.cache_vertex(
                            label,
                            internal_id,
                            external_id,
                            record.properties.clone(),
                            read_ts,
                            create_ts,
                            starts,
                        );
                    }
                }
            }
            Some(record)
        })
    }
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
        if external_id < 0 {
            return Err(StorageError::invalid_input(format!(
                "Vertex id cannot be negative: {}",
                external_id
            )));
        }
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

    /// Read a vertex record by internal ID, optionally restricted to a
    /// property projection. Never consults or populates the full-record cache
    /// (a projected read must not poison it with partial properties).
    fn read_record(
        &self,
        label: LabelId,
        internal_id: u32,
        projection: Option<&[String]>,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let gate = self.pending_gate();
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| -> Option<VertexRecord> {
                let table = vertex_tables.get(&label)?;
                Self::resolve_projected_on_table(table, internal_id, ts, projection, &gate)
            })
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

    pub fn get_external_id(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<String> {
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                vertex_tables
                    .get(&label)?
                    .get_external_id(internal_id, ts)
                    .map(|k| k.to_string())
            })
    }

    pub fn get_external_id_any(&self, internal_id: u32, ts: Timestamp) -> Option<String> {
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                vertex_tables
                    .values()
                    .find_map(|t| t.get_external_id(internal_id, ts))
                    .map(|k| k.to_string())
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
                Some(match key {
                    crate::vertex::IdKey::Int(i) => VertexId::from_int64(i),
                    crate::vertex::IdKey::Text(s) => VertexId::from_string(s),
                })
            })
    }

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

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                let internal_id = table
                    .get_internal_id(external_id, ts)
                    .ok_or(StorageError::vertex_not_found())?;
                table.update_property(internal_id, property_name, value, ts)?;
                Ok(internal_id)
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

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                let internal_id = table
                    .get_internal_id_by_i64(external_id, ts)
                    .ok_or(StorageError::vertex_not_found())?;
                table.update_property(internal_id, property_name, value, ts)?;
                Ok(internal_id)
            })?;

        self.persistent
            .cache_manager
            .remove_cached_vertex(label, internal_id);
        self.mark_vertex_modified(label);

        Ok(())
    }
}

#[cfg(test)]
mod revalidation_tests {
    use super::*;
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;

    fn setup_ctx() -> (GraphStorageContext, LabelId) {
        let ctx = GraphStorageContext::new();
        let label = ctx
            .create_vertex_type(
                "Person",
                vec![StoragePropertyDef::new(
                    "name".to_string(),
                    DataType::String,
                )],
                "name",
            )
            .expect("create vertex type");
        (ctx, label)
    }

    fn insert_person(
        ctx: &GraphStorageContext,
        label: LabelId,
        name: &str,
        value: &str,
        ts: Timestamp,
    ) {
        ctx.insert_vertex(
            label,
            name,
            &[("name".to_string(), Value::string(value))],
            ts,
        )
        .expect("insert vertex");
    }

    /// Acquire a real write timestamp from the version manager. Tests must
    /// never use synthetic timestamps: unwatermarked stamps bypass the
    /// read-frontier invariant the cache guards rely on.
    fn write_ts(ctx: &GraphStorageContext) -> Timestamp {
        ctx.persistent
            .version_manager
            .acquire_insert_timestamp()
            .expect("acquire write ts")
    }

    fn commit_ts(ctx: &GraphStorageContext, ts: Timestamp) {
        ctx.persistent
            .version_manager
            .commit_ordered(ts)
            .expect("ordered commit");
    }

    fn live_frontier(ctx: &GraphStorageContext) -> Timestamp {
        ctx.persistent.version_manager.read_timestamp()
    }

    fn read_name(ctx: &GraphStorageContext, label: LabelId, name: &str, ts: Timestamp) -> Value {
        ctx.get_vertex(label, name, ts)
            .expect("vertex must be visible")
            .properties
            .iter()
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v.clone())
            .expect("name property must be present")
    }

    fn internal_id_of(ctx: &GraphStorageContext, label: LabelId, name: &str, ts: Timestamp) -> u32 {
        ctx.persistent
            .data_store
            .with_vertex_tables(|tables| {
                tables.get(&label).and_then(|t| t.get_internal_id(name, ts))
            })
            .expect("internal id resolves")
    }

    #[test]
    fn stale_id_mapping_is_rejected_and_reseeded() {
        let (ctx, label) = setup_ctx();
        let ts = write_ts(&ctx);
        insert_person(&ctx, label, "alice", "A", ts);
        commit_ts(&ctx, ts);
        let frontier = live_frontier(&ctx);
        let real_id = internal_id_of(&ctx, label, "alice", frontier);
        assert!(ctx.get_vertex(label, "alice", frontier).is_some());

        // Poison the ID-index cache the way a racy seed after a GC remap
        // would: the mapping no longer matches the table.
        ctx.persistent
            .cache_manager
            .cache_vertex_id(label, "alice", real_id + 1000, frontier);
        let record = ctx
            .get_vertex(label, "alice", frontier)
            .expect("revalidation must fall back to the version-aware mapping");
        assert_eq!(record.internal_id, real_id);
        // The fresh mapping reseeds the cache.
        assert_eq!(
            ctx.persistent
                .cache_manager
                .get_cached_vertex_id(label, "alice", frontier),
            Some(real_id)
        );
    }

    #[test]
    fn stale_record_fence_returns_fresh_value() {
        let (ctx, label) = setup_ctx();
        let first = write_ts(&ctx);
        insert_person(&ctx, label, "alice", "A", first);
        commit_ts(&ctx, first);
        let frontier = live_frontier(&ctx);
        assert_eq!(
            read_name(&ctx, label, "alice", frontier),
            Value::string("A")
        );
        let internal_id = internal_id_of(&ctx, label, "alice", frontier);
        let seeded = ctx
            .persistent
            .cache_manager
            .get_cached_vertex(label, internal_id, frontier)
            .expect("seeded entry");
        assert_eq!(seeded.create_ts, first);

        // Concurrent commit lands after the seed (write path invalidates).
        let second = write_ts(&ctx);
        ctx.update_vertex_property(label, "alice", "name", &Value::string("B"), second)
            .expect("update");
        commit_ts(&ctx, second);

        // A racy seed publishes the pre-commit value after the invalidation.
        ctx.persistent.cache_manager.cache_vertex(
            label,
            internal_id,
            "alice".to_string(),
            seeded.properties.clone(),
            first,
            seeded.create_ts,
            seeded.column_starts.clone(),
        );

        // Hit revalidation must observe the drifted column fence and serve B.
        assert_eq!(
            read_name(&ctx, label, "alice", live_frontier(&ctx)),
            Value::string("B")
        );
    }

    #[test]
    fn historical_read_does_not_poison_future_readers() {
        let (ctx, label) = setup_ctx();
        let first = write_ts(&ctx);
        insert_person(&ctx, label, "alice", "A", first);
        commit_ts(&ctx, first);
        assert_eq!(read_name(&ctx, label, "alice", first), Value::string("A"));
        let second = write_ts(&ctx);
        ctx.update_vertex_property(label, "alice", "name", &Value::string("B"), second)
            .expect("update");
        commit_ts(&ctx, second);

        // The historical snapshot still reads A, but its version is no
        // longer current so the seed must be skipped.
        assert_eq!(read_name(&ctx, label, "alice", first), Value::string("A"));
        let internal_id = internal_id_of(&ctx, label, "alice", live_frontier(&ctx));
        assert!(
            ctx.persistent
                .cache_manager
                .get_cached_vertex(label, internal_id, live_frontier(&ctx))
                .is_none(),
            "stale version must not be seeded"
        );
        assert_eq!(
            read_name(&ctx, label, "alice", live_frontier(&ctx)),
            Value::string("B")
        );
    }

    #[test]
    fn concurrent_read_write_mix_stays_fresh() {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        use std::sync::Arc;

        let (ctx, label) = setup_ctx();
        let base = write_ts(&ctx);
        insert_person(&ctx, label, "key", "v0", base);
        commit_ts(&ctx, base);
        let done = Arc::new(AtomicBool::new(false));

        let writer_ctx = ctx.clone();
        let writer_done = done.clone();
        let writer = std::thread::spawn(move || {
            for i in 1..=50u64 {
                let ts = writer_ctx
                    .persistent
                    .version_manager
                    .acquire_insert_timestamp()
                    .expect("acquire write ts");
                writer_ctx
                    .update_vertex_property(
                        label,
                        "key",
                        "name",
                        &Value::string(format!("v{i}")),
                        ts,
                    )
                    .expect("writer update");
                writer_ctx
                    .persistent
                    .version_manager
                    .commit_ordered(ts)
                    .expect("ordered commit");
            }
            writer_done.store(true, AtomicOrdering::Release);
        });

        let mut readers = Vec::new();
        for _ in 0..3 {
            let reader_ctx = ctx.clone();
            let reader_done = done.clone();
            readers.push(std::thread::spawn(move || {
                let mut iterations = 0;
                while !reader_done.load(AtomicOrdering::Acquire) || iterations < 20 {
                    let frontier = reader_ctx.persistent.version_manager.read_timestamp();
                    let record = reader_ctx
                        .get_vertex(label, "key", frontier)
                        .expect("concurrent read must succeed");
                    let name = record
                        .properties
                        .iter()
                        .find(|(k, _)| k == "name")
                        .map(|(_, v)| v.clone())
                        .expect("name property must be present");
                    let stale = (0..=50u64).all(|i| name != Value::string(format!("v{i}")));
                    assert!(!stale, "no torn values under concurrency");
                    iterations += 1;
                    if iterations > 500 {
                        break;
                    }
                }
            }));
        }

        writer.join().expect("writer");
        for reader in readers {
            reader.join().expect("reader");
        }
        let frontier = ctx.persistent.version_manager.read_timestamp();
        assert_eq!(
            read_name(&ctx, label, "key", frontier),
            Value::string("v50")
        );
    }
}

#[cfg(test)]
mod pending_visibility_tests {
    use super::*;
    use crate::engine::{EdgeOperationParams, InsertEdgeParams};
    use crate::types::StoragePropertyDef;
    use crate::StorageOperationContext;
    use graphdb_core::types::{DataType, TransactionId};

    fn setup_ctx() -> (GraphStorageContext, LabelId) {
        let ctx = GraphStorageContext::new();
        let label = ctx
            .create_vertex_type(
                "Person",
                vec![StoragePropertyDef::new(
                    "name".to_string(),
                    DataType::String,
                )],
                "name",
            )
            .expect("create vertex type");
        (ctx, label)
    }

    fn bound_writer(
        ctx: &GraphStorageContext,
        txn: u64,
        read_ts: Timestamp,
        write_ts: Timestamp,
    ) -> GraphStorageContext {
        ctx.with_operation_context(StorageOperationContext::transaction_with_timestamps(
            TransactionId::from(txn),
            read_ts,
            Some(write_ts),
            false,
            false,
        ))
    }

    #[test]
    fn own_write_visible_point_and_projection() {
        let (ctx, label) = setup_ctx();
        let vm = ctx.persistent.version_manager.clone();
        let start = vm.acquire_insert_timestamp().expect("start txn");

        ctx.insert_vertex(
            label,
            "alice",
            &[("name".to_string(), Value::string("A"))],
            start,
        )
        .expect("insert own write");

        let bound = bound_writer(&ctx, 1, start, start);
        let record = bound
            .get_vertex(label, "alice", start)
            .expect("own write visible to point lookup");
        assert_eq!(
            record
                .properties
                .iter()
                .find(|(k, _)| k == "name")
                .map(|(_, v)| v),
            Some(&Value::string("A"))
        );
        let projected = bound
            .get_vertex_projected(label, "alice", &["name".to_string()], start)
            .expect("own write visible to projection");
        assert_eq!(
            projected
                .properties
                .iter()
                .find(|(k, _)| k == "name")
                .map(|(_, v)| v),
            Some(&Value::string("A"))
        );
        vm.commit_ordered(start).expect("ordered commit");
    }

    #[test]
    fn concurrent_write_transactions_hide_each_other_pending_writes() {
        let (ctx, label) = setup_ctx();
        let vm = ctx.persistent.version_manager.clone();
        let first = vm.acquire_insert_timestamp().expect("first txn");
        ctx.insert_vertex(
            label,
            "alice",
            &[("name".to_string(), Value::string("A"))],
            first,
        )
        .expect("first txn writes");

        let second = vm.acquire_insert_timestamp().expect("second txn");
        let bound_second = bound_writer(&ctx, 2, second, second);
        assert!(
            bound_second.get_vertex(label, "alice", second).is_none(),
            "concurrent writer must not observe the foreign uncommitted row"
        );

        // After the first transaction commits, the same snapshot observes it:
        // the committed stamp is at or below the advanced frontier.
        vm.commit_ordered(first).expect("ordered commit");
        assert!(
            bound_second.get_vertex(label, "alice", second).is_some(),
            "committed row becomes visible to the peer snapshot"
        );

        // A fresh snapshot after both settle observes the row as well.
        vm.abort_write_timestamp(second);
        let frontier = vm.read_timestamp();
        let bound_read =
            ctx.with_operation_context(StorageOperationContext::transaction_with_timestamps(
                TransactionId::from(3),
                frontier,
                None,
                true,
                false,
            ));
        assert!(
            bound_read.get_vertex(label, "alice", frontier).is_some(),
            "row stays visible across frontier advance"
        );
    }

    #[test]
    fn own_edge_write_visible_point_and_traversal() {
        use crate::edge::EdgeStrategy;

        let ctx = GraphStorageContext::new();
        let src_label = ctx
            .create_vertex_type(
                "Person",
                vec![StoragePropertyDef::new(
                    "name".to_string(),
                    DataType::String,
                )],
                "name",
            )
            .expect("src label");
        let dst_label = ctx
            .create_vertex_type(
                "City",
                vec![StoragePropertyDef::new(
                    "name".to_string(),
                    DataType::String,
                )],
                "name",
            )
            .expect("dst label");
        let edge_label = ctx
            .create_edge_type(
                "LIVES_IN",
                src_label,
                dst_label,
                vec![],
                EdgeStrategy::Multiple,
                EdgeStrategy::Multiple,
            )
            .expect("edge type");

        let vm = ctx.persistent.version_manager.clone();
        let start = vm.acquire_insert_timestamp().expect("start txn");
        ctx.insert_vertex_by_i64(
            src_label,
            1,
            &[("name".to_string(), Value::string("A"))],
            start,
        )
        .expect("src vertex");
        ctx.insert_vertex_by_i64(
            dst_label,
            2,
            &[("name".to_string(), Value::string("B"))],
            start,
        )
        .expect("dst vertex");
        ctx.insert_edge(InsertEdgeParams {
            edge_label,
            src_label,
            src_id: VertexId::from_int64(1),
            dst_label,
            dst_id: VertexId::from_int64(2),
            rank: 0,
            properties: &[],
            ts: start,
        })
        .expect("own edge write");

        let bound = bound_writer(&ctx, 1, start, start);
        let params = EdgeOperationParams {
            edge_label,
            src_label,
            src_id: VertexId::from_int64(1),
            dst_label,
            dst_id: VertexId::from_int64(2),
            rank: 0,
        };
        assert!(
            bound.get_edge(&params, start).is_some(),
            "own edge write visible to point lookup"
        );
        let neighbors = bound
            .out_edges(
                edge_label,
                src_label,
                dst_label,
                VertexId::from_int64(1),
                start,
            )
            .expect("traversal resolves");
        assert_eq!(neighbors.len(), 1);

        // A concurrent writer still pending must not observe the edge.
        let peer = vm.acquire_insert_timestamp().expect("peer txn");
        let bound_peer = bound_writer(&ctx, 2, peer, peer);
        assert!(
            bound_peer.get_edge(&params, peer).is_none(),
            "concurrent writer must not observe the foreign uncommitted edge"
        );
        vm.commit_ordered(start).expect("ordered commit");
        vm.abort_write_timestamp(peer);
    }

    #[test]
    fn scan_and_projection_hide_foreign_pending_row() {
        let (ctx, label) = setup_ctx();
        let vm = ctx.persistent.version_manager.clone();
        let first = vm.acquire_insert_timestamp().expect("first txn");
        ctx.insert_vertex(
            label,
            "alice",
            &[("name".to_string(), Value::string("A"))],
            first,
        )
        .expect("first txn writes");

        let second = vm.acquire_insert_timestamp().expect("second txn");
        let bound_second = bound_writer(&ctx, 2, second, second);
        assert!(
            bound_second
                .get_vertex_projected(label, "alice", &["name".to_string()], second)
                .is_none(),
            "projection must not observe the foreign uncommitted row"
        );
        let scanned = bound_second
            .scan_vertices(label, second)
            .expect("scan resolves");
        assert!(
            scanned.is_empty(),
            "scan must not observe the foreign uncommitted row"
        );

        vm.commit_ordered(first).expect("ordered commit");
        assert!(
            bound_second
                .get_vertex_projected(label, "alice", &["name".to_string()], second)
                .is_some(),
            "projection observes the row after commit"
        );
        let scanned = bound_second
            .scan_vertices(label, second)
            .expect("scan resolves");
        assert_eq!(scanned.len(), 1);
        vm.abort_write_timestamp(second);
    }
}
