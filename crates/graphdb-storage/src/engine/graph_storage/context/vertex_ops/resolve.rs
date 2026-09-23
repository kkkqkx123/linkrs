use crate::engine::cache_manager::VertexSeed;
use crate::mvcc_visibility::PendingGate;
use crate::vertex::{ShardedVertexTable, VertexRecord};
use graphdb_core::types::{LabelId, Timestamp};

use super::super::GraphStorageContext;

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
    /// Accepts the cached record only when the row is still alive at `ts`,
    /// its creation stamp matches the seed fence, and the per-column
    /// covering stamps match (no property write landed between seed and
    /// query). Anything else is a miss.
    pub(super) fn rechecked_cached_record(
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
                .ok()
                .and_then(|parsed| {
                    graphdb_core::types::VertexId::try_from_int64(parsed).ok()
                })
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
        let gate = self.pending_gate();
        self.persistent
            .data_store
            .with_vertex_tables(|vertex_tables| -> Option<VertexRecord> {
                let table = vertex_tables.get(&label)?;
                Self::resolve_projected_on_table(table, internal_id, ts, projection, &gate)
            })
    }
}
