//! Transaction staging composition for vertex reads.
//!
//! Every vertex read that can observe this transaction's own writes goes
//! through the helpers on `GraphStorageContext` in this file: delete vetoes,
//! staged insert records, scan merges and full record composition. Direct
//! staging buffer access outside this file is limited to id resolution,
//! serial conflict scans and emptiness probes, which never compose row
//! content.
use crate::engine::cache_manager::VertexSeed;
use crate::mvcc_visibility::{PendingGate, VisibilityGuard};
use crate::vertex::{IdKey, ShardedVertexTable, VertexRecord};
use graphdb_core::types::{LabelId, Timestamp, VertexId};

use super::super::GraphStorageContext;

/// External key of a vertex id, in the staging key space.
pub(crate) fn id_key_of(vid: &VertexId) -> Option<IdKey> {
    vid.as_int64()
        .map(IdKey::Int)
        .or_else(|| vid.as_str().map(|s| IdKey::Text(s.to_string())))
}

/// Vertex id of an external key (inverse of [`id_key_of`]).
pub(crate) fn vid_of_key(key: &IdKey) -> Option<VertexId> {
    match key {
        IdKey::Int(i) => VertexId::try_from_int64(*i).ok(),
        IdKey::Text(s) => VertexId::try_from_string(s).ok(),
    }
}

/// Overlay staged column updates onto a property list: later-staged values
/// win, columns absent from the base row are appended.
pub(crate) fn apply_staged_columns(
    properties: &mut Vec<(String, graphdb_core::Value)>,
    columns: &[(String, graphdb_core::Value)],
) {
    for (name, value) in columns {
        match properties.iter_mut().find(|(existing, _)| existing == name) {
            Some(slot) => slot.1 = value.clone(),
            None => properties.push((name.clone(), value.clone())),
        }
    }
}

/// A row surfaced by a transaction's own staging: composed property list
/// plus the external key and the id to present (the reserved global id
/// staged at insert time, or the real internal id for composed updates).
pub(crate) struct StagedRow {
    pub vid: VertexId,
    pub id: u32,
    pub properties: Vec<(String, graphdb_core::Value)>,
}

/// Per-label scan merge input derived from staging.
pub(crate) struct StagedScanMerge {
    /// Global ids whose rows must not be yielded (staged deletes) or are
    /// superseded by a composed staged update row.
    pub dropped_ids: std::collections::HashSet<u32>,
    /// Composed rows appended to the scan: staged inserts (reserved global
    /// id) and staged updates (real internal id).
    pub rows: Vec<StagedRow>,
}

impl GraphStorageContext {
    /// Filter a property list through an optional projection.
    fn project_properties(
        properties: &[(String, graphdb_core::Value)],
        projection: Option<&[String]>,
    ) -> Vec<(String, graphdb_core::Value)> {
        match projection {
            None => properties.to_vec(),
            Some(names) => properties
                .iter()
                .filter(|(name, _)| names.iter().any(|n| n == name))
                .cloned()
                .collect(),
        }
    }

    /// Whether the active transaction has staged a delete for this key.
    /// Used by id-resolution entries so a self-deleted vertex stops
    /// resolving before the tombstone lands.
    pub(crate) fn staged_delete_vetoes(&self, label: LabelId, key: &IdKey) -> bool {
        self.active_txn_staging()
            .is_some_and(|buffer| buffer.lock().has_pending_delete(label, key))
    }

    /// Projected view of a staged insert row, surfaced when the key has no
    /// committed binding yet (the row only exists in this transaction's
    /// staging, under its reserved global id).
    pub(crate) fn staged_insert_record(
        &self,
        label: LabelId,
        key: &IdKey,
        projection: Option<&[String]>,
    ) -> Option<VertexRecord> {
        let buffer = self.active_txn_staging()?;
        let buffer = buffer.lock();
        let (reserved, staged) = buffer.pending_insert_row(label, key)?;
        let properties = Self::project_properties(staged, projection);
        Some(VertexRecord {
            internal_id: reserved,
            vid: vid_of_key(key)?,
            properties,
        })
    }

    /// Merge inputs for a label-table scan: staged deletes and updates
    /// retire their global ids (updates re-emit composed), staged inserts
    /// append as new rows. Reads global bases under the caller's guard.
    pub(crate) fn staged_scan_merge(
        &self,
        label: LabelId,
        table: &ShardedVertexTable,
        guard: &VisibilityGuard<'_>,
    ) -> StagedScanMerge {
        let mut merge = StagedScanMerge {
            dropped_ids: std::collections::HashSet::new(),
            rows: Vec::new(),
        };
        let Some(buffer) = self.active_txn_staging() else {
            return merge;
        };
        let buffer = buffer.lock();
        // Deletes and updates address rows by their cached resolution; a
        // missing resolution means the key was staged without an existing
        // row, which a scan cannot contain.
        for (key, columns) in buffer.update_rows(label) {
            let Some(id) = buffer.resolve(label, key) else {
                continue;
            };
            merge.dropped_ids.insert(id);
            let Some(base) = table.resolve_projected(id, guard, None) else {
                continue;
            };
            let Some(vid) = vid_of_key(key) else {
                continue;
            };
            let mut properties = base.properties;
            apply_staged_columns(&mut properties, columns);
            merge.rows.push(StagedRow {
                vid,
                id,
                properties,
            });
        }
        for key in buffer.delete_keys(label) {
            if let Some(id) = buffer.resolve(label, key) {
                merge.dropped_ids.insert(id);
            }
        }
        for (key, reserved, properties) in buffer.insert_rows(label) {
            let Some(vid) = vid_of_key(key) else {
                continue;
            };
            merge.rows.push(StagedRow {
                vid,
                id: reserved,
                properties: properties.to_vec(),
            });
        }
        merge
    }

    /// Compose a full record-cache-eligible point lookup with staging after
    /// the global read. Kept separate from cache handling: a composed row
    /// never consults or seeds the record cache (the cache is committed
    /// state only).
    pub(crate) fn compose_full_record(
        &self,
        label: LabelId,
        key: &IdKey,
        global: Option<VertexRecord>,
    ) -> Option<VertexRecord> {
        let Some(buffer) = self.active_txn_staging() else {
            return global;
        };
        let properties = global.as_ref().map(|record| record.properties.clone());
        let global_id = global.as_ref().map(|record| record.internal_id);
        let composed: Option<Option<(u32, Vec<(String, graphdb_core::Value)>)>> = {
            let buffer = buffer.lock();
            if buffer.has_pending_delete(label, key) {
                Some(None)
            } else if let Some((reserved, props)) = buffer.pending_insert_row(label, key) {
                Some(Some((reserved, props.to_vec())))
            } else if let Some(columns) = buffer.pending_update(label, key) {
                match properties {
                    Some(mut merged) => {
                        apply_staged_columns(&mut merged, columns);
                        Some(global_id.map(|internal_id| (internal_id, merged)))
                    }
                    None => Some(None),
                }
            } else {
                None
            }
        };
        let Some(composed) = composed else {
            return global;
        };
        let Some((internal_id, properties)) = composed else {
            return None;
        };
        Some(VertexRecord {
            internal_id,
            vid: vid_of_key(key)?,
            properties,
        })
    }
}

/// External vertex reference for point lookups.
#[derive(Clone, Copy)]
pub(super) enum ExternalRef<'a> {
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

    fn id_key(&self) -> IdKey {
        match *self {
            ExternalRef::Str(id) => IdKey::Text(id.to_string()),
            ExternalRef::I64(id) => IdKey::Int(id),
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

    /// The read snapshot bound to this context's pending gate. Every vertex
    /// enumeration and record read takes one, so a read path cannot omit the
    /// pending recheck.
    pub(crate) fn visibility_guard(&self, ts: Timestamp) -> VisibilityGuard<'_> {
        VisibilityGuard::new(ts, self.pending_gate())
    }

    /// Owned gate inputs for closures that mutate their owner while
    /// filtering: the caller clones the manager handle up front and builds
    /// the guard inside the closure so no borrow of `self` is retained.
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
    pub(super) fn resolve_internal_id_rechecked(
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
    /// Accepts the cached record only when the row is still visible to
    /// `guard`, its creation stamp matches the seed fence, and the per-column
    /// covering stamps match (no property write landed between seed and
    /// query). Anything else is a miss. The pending-aware check matters
    /// because a cache entry can be seeded by the writer that owns an
    /// uncommitted version: a foreign reader at a later snapshot must miss,
    /// not read that version through the back door.
    pub(super) fn rechecked_cached_record(
        &self,
        table: &ShardedVertexTable,
        label: LabelId,
        internal_id: u32,
        guard: &VisibilityGuard<'_>,
    ) -> Option<VertexRecord> {
        let cached = self.persistent.cache_manager.get_cached_vertex(
            label,
            internal_id,
            guard.snapshot(),
        )?;
        let (create_ts, delete_ts) = table.row_timestamps(internal_id)?;
        if create_ts != cached.create_ts {
            return None;
        }
        if !guard.is_row_visible(create_ts, delete_ts) {
            return None;
        }
        if table.row_picked_starts(internal_id, guard.snapshot()) != cached.column_starts {
            return None;
        }
        Some(VertexRecord {
            internal_id: cached.internal_id,
            vid: cached
                .external_id
                .parse::<i64>()
                .ok()
                .and_then(|parsed| graphdb_core::types::VertexId::try_from_int64(parsed).ok())
                .or_else(|| {
                    graphdb_core::types::VertexId::try_from_string(&cached.external_id).ok()
                })?,
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
    pub(super) fn get_full_record(
        &self,
        label: LabelId,
        external: ExternalRef<'_>,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let key = external.id_key();
        let global = self.get_full_record_global(label, external, ts);
        self.compose_full_record(label, &key, global)
    }

    fn get_full_record_global(
        &self,
        label: LabelId,
        external: ExternalRef<'_>,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let guard = self.visibility_guard(ts);
        self.persistent.data_store.with_vertex_tables(|tables| {
            let table = tables.get(&label)?;
            let internal_id = self.resolve_internal_id_rechecked(table, label, external, ts)?;
            if self.record_cache_eligible(ts) {
                if let Some(hit) = self.rechecked_cached_record(table, label, internal_id, &guard) {
                    return Some(hit);
                }
            }
            let (record, create_ts, starts, read_ts) = table.resolve_vertex(internal_id, &guard)?;
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
                        VertexSeed {
                            external_id: &external.cache_key(),
                            properties: &record.properties,
                            read_ts,
                            create_ts,
                            column_starts: &starts,
                        },
                    );
                }
            }
            Some(record)
        })
    }

    /// Full point lookup by internal id, with the same hit revalidation and
    /// conditional seeding as [`Self::get_full_record`].
    pub(super) fn get_full_record_by_internal_id(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let global = self.get_full_record_by_internal_id_global(label, internal_id, ts);
        match self.staged_key_for_id(label, internal_id) {
            Some(key) => self.compose_full_record(label, &key, global),
            None => global,
        }
    }

    fn get_full_record_by_internal_id_global(
        &self,
        label: LabelId,
        internal_id: u32,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let guard = self.visibility_guard(ts);
        self.persistent.data_store.with_vertex_tables(|tables| {
            let table = tables.get(&label)?;
            if self.record_cache_eligible(ts) {
                if let Some(hit) = self.rechecked_cached_record(table, label, internal_id, &guard) {
                    return Some(hit);
                }
            }
            let (record, create_ts, starts, read_ts) = table.resolve_vertex(internal_id, &guard)?;
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
                        .get_external_id_raw(internal_id)
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
                            VertexSeed {
                                external_id: &external_id,
                                properties: &record.properties,
                                read_ts,
                                create_ts,
                                column_starts: &starts,
                            },
                        );
                    }
                }
            }
            Some(record)
        })
    }

    /// Read a vertex record by internal ID, optionally restricted to a
    /// property projection. Never consults or populates the full-record cache
    /// (a projected read must not poison it with partial properties).
    pub(super) fn read_record(
        &self,
        label: LabelId,
        internal_id: u32,
        projection: Option<&[String]>,
        ts: Timestamp,
    ) -> Option<VertexRecord> {
        let key = self.staged_key_for_id(label, internal_id);
        if let Some(key) = &key {
            if self.staged_delete_vetoes(label, key) {
                return None;
            }
        }
        let guard = self.visibility_guard(ts);
        let record = self.persistent.data_store.with_vertex_tables(
            |vertex_tables| -> Option<VertexRecord> {
                let table = vertex_tables.get(&label)?;
                table.resolve_projected(internal_id, &guard, projection)
            },
        )?;
        if let (Some(key), Some(buffer)) = (key, self.active_txn_staging()) {
            let buffer = buffer.lock();
            if let Some(columns) = buffer.pending_update(label, &key) {
                let mut properties = record.properties;
                match projection {
                    None => apply_staged_columns(&mut properties, columns),
                    Some(names) => {
                        let scoped: Vec<_> = columns
                            .iter()
                            .filter(|(name, _)| names.iter().any(|n| n == name))
                            .cloned()
                            .collect();
                        apply_staged_columns(&mut properties, &scoped);
                    }
                }
                return Some(VertexRecord {
                    properties,
                    ..record
                });
            }
        }
        Some(record)
    }

    /// External key currently bound to an internal id in one label table.
    pub(crate) fn staged_key_for_id(&self, label: LabelId, internal_id: u32) -> Option<IdKey> {
        self.persistent
            .data_store
            .with_vertex_tables(|tables| tables.get(&label)?.get_external_id_raw(internal_id))
    }
}
