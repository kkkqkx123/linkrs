use super::super::core::VertexTable;
use super::routing::decode_id;
use super::ShardedVertexTable;
use crate::cursor::ColumnValues;
use crate::mvcc_visibility::VisibilityGuard;
use crate::vertex::{IdKey, PkLookup, VertexRecord};
use graphdb_core::types::{DataType, Timestamp, VertexId};

/// Decode an ID-index key into the external vertex ID.
///
/// Keys are length-checked at insert, so a decode failure surfaces as a
/// missing row under the existing absence contract.
fn vertex_id_of(key: IdKey) -> Option<VertexId> {
    match key {
        IdKey::Int(i) => VertexId::try_from_int64(i).ok(),
        IdKey::Text(s) => VertexId::try_from_string(&s).ok(),
    }
}

impl ShardedVertexTable {
    // ── Plain timestamp primitives ──
    //
    // These apply the timestamp predicate only and take no visibility guard,
    // so they can read a version a guard would hide. They stay crate-private
    // and exist for the offline/startup paths that run with no transaction in
    // flight (WAL replay, reshard). Every entry point that hands row identity
    // or row data to a consumer takes a [`VisibilityGuard`] instead.

    pub(crate) fn get_by_internal_id(&self, global_id: u32, ts: Timestamp) -> Option<VertexRecord> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.get_by_internal_id(local_id, ts).map(|mut record| {
            record.internal_id = global_id;
            record
        })
    }

    /// Row survival stamps for visibility rechecks (shard-decoded).
    pub(crate) fn row_timestamps(&self, global_id: u32) -> Option<(Timestamp, Option<Timestamp>)> {
        let (idx, local_id) = self.decode_id(global_id);
        self.shards[idx].read().row_timestamps(local_id)
    }

    /// Per-column covering version stamps for visibility rechecks.
    pub(crate) fn row_picked_starts(&self, global_id: u32, ts: Timestamp) -> Vec<Timestamp> {
        let (idx, local_id) = self.decode_id(global_id);
        self.shards[idx].read().row_picked_starts(local_id, ts)
    }

    pub(crate) fn get_external_id(&self, global_id: u32, ts: Timestamp) -> Option<IdKey> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.get_external_id(local_id, ts)
    }

    // ── Guarded reads ──

    /// Whether one shard row is live for `guard` at its snapshot.
    ///
    /// Vertex rows reach the main table only at their transaction's commit
    /// application (or through offline paths with no transaction in flight),
    /// so the guard's liveness check at the snapshot is the full resolution:
    /// there is no uncommitted version chain to walk below. The pending
    /// check also covers a writer interleaving between the identity read and
    /// the segment read, so point reads need no stamp revalidation loop.
    fn shard_row_visible(table: &VertexTable, local_id: u32, guard: &VisibilityGuard<'_>) -> bool {
        table
            .row_timestamps(local_id)
            .is_some_and(|(create_ts, delete_ts)| guard.is_row_visible(create_ts, delete_ts))
    }

    /// Guarded full point read, with the fences describing the version read:
    /// creation stamp, per-column covering stamps and the read stamp. The
    /// record cache fences on these.
    pub(crate) fn resolve_vertex(
        &self,
        global_id: u32,
        guard: &VisibilityGuard<'_>,
    ) -> Option<(VertexRecord, Timestamp, Vec<Timestamp>, Timestamp)> {
        let (shard_idx, local_id) = self.decode_id(global_id);
        let table = self.shards[shard_idx].read();
        let (create_ts, delete_ts) = table.row_timestamps(local_id)?;
        if !guard.is_row_visible(create_ts, delete_ts) {
            return None;
        }
        let snapshot = guard.snapshot();
        let mut record = table.get_projected_by_internal_id(local_id, snapshot, None)?;
        record.internal_id = global_id;
        let starts = table.row_picked_starts(local_id, snapshot);
        Some((record, create_ts, starts, snapshot))
    }

    /// Guarded projected point read. Decodes the projection at the snapshot.
    pub fn resolve_projected(
        &self,
        global_id: u32,
        guard: &VisibilityGuard<'_>,
        projection: Option<&[String]>,
    ) -> Option<VertexRecord> {
        let (shard_idx, local_id) = self.decode_id(global_id);
        let table = self.shards[shard_idx].read();
        if !Self::shard_row_visible(&table, local_id, guard) {
            return None;
        }
        table
            .get_projected_by_internal_id(local_id, guard.snapshot(), projection)
            .map(|mut record| {
                record.internal_id = global_id;
                record
            })
    }

    /// Batch variant of [`Self::resolve_projected`].
    ///
    /// Input ids are grouped by shard, resolved with one lock acquisition per
    /// shard and decoded in one column-major batch. The output is aligned with
    /// the input order; rows with no visible version yield `None`.
    pub fn resolve_projected_batch(
        &self,
        global_ids: &[u32],
        guard: &VisibilityGuard<'_>,
        projection: Option<&[String]>,
    ) -> Vec<Option<VertexRecord>> {
        let snapshot = guard.snapshot();
        let mut out: Vec<Option<VertexRecord>> = global_ids.iter().map(|_| None).collect();
        for (shard_idx, group) in self.group_by_shard(global_ids) {
            if group.is_empty() {
                continue;
            }
            let table = self.shards[shard_idx].read();
            let visible: Vec<(usize, u32)> = group
                .into_iter()
                .filter(|&(_, local_id)| Self::shard_row_visible(&table, local_id, guard))
                .collect();
            let locals: Vec<u32> = visible.iter().map(|&(_, local)| local).collect();
            let records = table.get_projected_batch(&locals, snapshot, projection);
            for ((slot, _), record) in visible.into_iter().zip(records) {
                out[slot] = record.map(|mut record| {
                    record.internal_id = self.encode_id(shard_idx, record.internal_id);
                    record
                });
            }
        }
        out
    }

    /// Full cross-shard scan at the guard's snapshot.
    ///
    /// Each shard is scanned under its own read lock and the per-shard results
    /// are concatenated in shard order, so concurrent writes may be observed
    /// inconsistently across shards. Point lookups stay shard-consistent.
    pub fn scan(&self, guard: &VisibilityGuard<'_>) -> Vec<VertexRecord> {
        use rayon::prelude::*;
        let snapshot = guard.snapshot();
        let per_shard: Vec<(usize, Vec<VertexRecord>)> = self
            .shards
            .par_iter()
            .enumerate()
            .map(|(shard_idx, shard)| {
                let table = shard.read();
                let mut records: Vec<VertexRecord> = table
                    .scan(snapshot)
                    .filter_map(|mut record| {
                        let local_id = record.internal_id;
                        if !Self::shard_row_visible(&table, local_id, guard) {
                            return None;
                        }
                        record.internal_id = self.encode_id(shard_idx, local_id);
                        Some(record)
                    })
                    .collect();
                records.sort_by_key(|record| record.internal_id);
                (shard_idx, records)
            })
            .collect();
        // Shards are independent read domains: parallel scan is safe, and
        // results are reassembled in shard order for stable pagination.
        let mut ordered = vec![Vec::new(); per_shard.len()];
        for (shard_idx, records) in per_shard {
            ordered[shard_idx] = records;
        }
        ordered.into_iter().flatten().collect()
    }

    /// Candidate id enumeration for paginated scans: rows the guard considers
    /// visible at its snapshot.
    ///
    /// Shards are read without a global lock, so concurrent writes may be
    /// observed inconsistently across shards. Shards decode in parallel and
    /// reassemble in shard order, matching [`Self::scan`].
    pub fn live_ids(&self, guard: &VisibilityGuard<'_>) -> Vec<u32> {
        use rayon::prelude::*;
        let snapshot = guard.snapshot();
        let per_shard: Vec<(usize, Vec<u32>)> = self
            .shards
            .par_iter()
            .enumerate()
            .map(|(shard_idx, shard)| {
                let table = shard.read();
                let mut shard_ids: Vec<u32> = table
                    .live_ids(snapshot)
                    .into_iter()
                    .filter(|&local_id| Self::shard_row_visible(&table, local_id, guard))
                    .map(|local_id| self.encode_id(shard_idx, local_id))
                    .collect();
                shard_ids.sort_unstable();
                (shard_idx, shard_ids)
            })
            .collect();
        let mut ordered = vec![Vec::new(); per_shard.len()];
        for (shard_idx, shard_ids) in per_shard {
            ordered[shard_idx] = shard_ids;
        }
        ordered.into_iter().flatten().collect()
    }

    /// Column-major batch decode for paginated scans.
    ///
    /// Filters candidates by guard visibility, decodes the requested columns
    /// at the snapshot (a full decode when `names` is empty) and compacts the
    /// result. The returned ids, external vertex ids and columns are aligned;
    /// rows with no visible version are dropped.
    pub fn scan_columns(
        &self,
        global_ids: &[u32],
        guard: &VisibilityGuard<'_>,
        names: &[String],
    ) -> (Vec<u32>, Vec<VertexId>, Vec<(String, ColumnValues)>) {
        let snapshot = guard.snapshot();
        let (resolved_names, types) = self.column_layout(names);
        let mut merged: Vec<(String, ColumnValues)> = resolved_names
            .iter()
            .zip(types.iter())
            .map(|(name, data_type)| {
                (
                    name.clone(),
                    empty_typed_column(data_type.as_ref(), global_ids.len()),
                )
            })
            .collect();

        let mut vids: Vec<Option<VertexId>> = vec![None; global_ids.len()];
        for (shard_idx, group) in self.group_by_shard(global_ids) {
            if group.is_empty() {
                continue;
            }
            let table = self.shards[shard_idx].read();
            let mut visible: Vec<(usize, u32)> = Vec::new();
            for (slot, local_id) in group {
                if !Self::shard_row_visible(&table, local_id, guard) {
                    continue;
                }
                let Some(vid) = table.get_external_id_raw(local_id).and_then(vertex_id_of) else {
                    continue;
                };
                vids[slot] = Some(vid);
                visible.push((slot, local_id));
            }
            if visible.is_empty() {
                continue;
            }
            let locals: Vec<u32> = visible.iter().map(|&(_, local)| local).collect();
            for (name, column) in table.get_projected_columns(&locals, snapshot, &resolved_names) {
                if let Some((_, target)) = merged.iter_mut().find(|(n, _)| *n == name) {
                    column.scatter(target, &visible);
                }
            }
        }

        // Compact to the surviving rows: the decode was pre-sized to the
        // candidate count, which includes rows the guard hid.
        let mut kept_ids = Vec::new();
        let mut kept_vids = Vec::new();
        let mut selection = Vec::new();
        for (slot, vid) in vids.into_iter().enumerate() {
            let Some(vid) = vid else { continue };
            kept_ids.push(global_ids[slot]);
            kept_vids.push(vid);
            selection.push(slot);
        }
        if selection.len() != global_ids.len() {
            for (_, column) in merged.iter_mut() {
                column.select(&selection);
            }
        }

        // Already-typed merges skip the box-and-retype roundtrip; only
        // `General` columns (unknown type or cross-shard kind mismatch)
        // attempt recovery through the declared type.
        for (index, data_type) in types.into_iter().enumerate() {
            if !matches!(merged[index].1, ColumnValues::General(_)) {
                continue;
            }
            if let Some(data_type) = data_type {
                let general = merged[index].1.to_general();
                if let Some(typed) = ColumnValues::from_general_with_type(general, &data_type) {
                    merged[index].1 = typed;
                }
            }
        }
        (kept_ids, kept_vids, merged)
    }

    /// Group input slots by shard so each shard is locked once per batch.
    ///
    /// The result is indexed by shard order; the tuple pairs the output slot
    /// with the shard-local id.
    fn group_by_shard(&self, global_ids: &[u32]) -> Vec<(usize, Vec<(usize, u32)>)> {
        let mut by_shard: Vec<Vec<(usize, u32)>> = vec![Vec::new(); self.layout.num_shards];
        for (slot, &global_id) in global_ids.iter().enumerate() {
            let (shard_idx, local_id) = decode_id(global_id, self.layout);
            by_shard[shard_idx].push((slot, local_id));
        }
        by_shard.into_iter().enumerate().collect()
    }

    /// Column names and declared types for a decode request. An empty request
    /// means every column of the table.
    fn column_layout(&self, names: &[String]) -> (Vec<String>, Vec<Option<DataType>>) {
        let table = self.shards[0].read();
        let resolved_names: Vec<String> = if names.is_empty() {
            table
                .schema()
                .properties
                .iter()
                .map(|p| p.name.clone())
                .collect()
        } else {
            names.to_vec()
        };
        let types = resolved_names
            .iter()
            .map(|name| table.data_type_of(name))
            .collect();
        (resolved_names, types)
    }

    /// Zone-map pruning mask over `ids` (global internal ids).
    ///
    /// `mask[i] == false` means the row's zone-map chunk provably cannot
    /// contain values matching any of `ranges`, so the id can be skipped
    /// before decoding. Unknown columns and chunks without bounds keep the
    /// id (conservative). Complex equality probes first prune on the
    /// per-chunk length summary, then fall back to whole-value ordering.
    pub fn zone_prune_mask(
        &self,
        ids: &[u32],
        ranges: &[crate::cursor::PredicateRange],
    ) -> Vec<bool> {
        let mut mask = vec![true; ids.len()];
        if ranges.is_empty() {
            return mask;
        }
        for (shard_idx, group) in self.group_by_shard(ids) {
            if group.is_empty() {
                continue;
            }
            let table = self.shards[shard_idx].read();
            for (slot, local_id) in group {
                let chunk = local_id as usize / crate::vertex::column_store::ZONE_MAP_CHUNK_ROWS;
                for range in ranges {
                    if !table.columns.zone_prunes_in(chunk, range) {
                        mask[slot] = false;
                        break;
                    }
                }
            }
        }
        mask
    }

    /// Aggregate optimizer-facing statistics for one property column across
    /// all shards. Zone-map bounds are merged with the numeric-aware
    /// comparison used by pushed predicates; null/distinct counts come from
    /// the persisted column stats meta when available. Returns `None` when
    /// the column is unknown or no shard has any recorded information.
    ///
    /// `row_count` is the live row estimate at `ts`, not the allocated
    /// slot total: deleted-but-unreclaimed rows must not inflate the
    /// optimizer's cardinality.
    pub fn column_stats_snapshot_at(
        &self,
        column: &str,
        ts: Timestamp,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        use crate::stats_reader::ColumnStatsSnapshot;

        let mut min: Option<graphdb_core::Value> = None;
        let mut max: Option<graphdb_core::Value> = None;
        let mut null_count: Option<u64> = None;
        let mut merged_hll: Option<crate::stats::HyperLogLog> = None;
        let mut hll_complete = true;
        let mut any_info = false;

        for shard in &self.shards {
            let table = shard.read();
            if let Some(bounds) = table.columns.aggregate_zone_bounds(column) {
                any_info = true;
                crate::stats_reader::merge_min(&mut min, bounds.min);
                crate::stats_reader::merge_max(&mut max, bounds.max);
            }
            if let Some(stats) = table.columns.get_column(column).and_then(|c| c.stats()) {
                any_info = true;
                *null_count.get_or_insert(0) += stats.null_count;
                match &stats.hll {
                    Some(h) => {
                        if let Some(ref mut acc) = merged_hll {
                            acc.merge(h);
                        } else {
                            merged_hll = Some(h.clone());
                        }
                    }
                    None => {
                        hll_complete = false;
                    }
                }
            }
        }

        if !any_info {
            return None;
        }
        let (hll, distinct_count) = match (merged_hll, hll_complete) {
            (Some(h), true) => {
                let est = h.estimate();
                (Some(h), Some(est))
            }
            _ => (None, None),
        };
        Some(ColumnStatsSnapshot {
            row_count: self.approximate_id_hole_stats(ts).0 as u64,
            null_count,
            distinct_count,
            hll,
            min_value: min,
            max_value: max,
        })
    }

    /// Table-level cardinality snapshot for the optimizer.
    ///
    /// Wraps [`Self::approximate_id_hole_stats`] in the shared
    /// [`crate::stats_reader::TableCardinalitySnapshot`] shape so the query
    /// layer no longer reassembles live versus allocated counts itself.
    /// Shard-inconsistent like the underlying counts; for sizing and plan
    /// costing only.
    pub fn table_cardinality_at(
        &self,
        ts: Timestamp,
    ) -> crate::stats_reader::TableCardinalitySnapshot {
        let (live, allocated) = self.approximate_id_hole_stats(ts);
        crate::stats_reader::TableCardinalitySnapshot {
            live_rows: live as u64,
            allocated_slots: allocated as u64,
            shard_count: self.layout.num_shards,
        }
    }

    // ── Identity and sizing ──
    //
    // These resolve row identity or counts rather than row content, so they
    // keep the plain timestamp predicate.

    pub fn get_internal_id(&self, external_id: &str, ts: Timestamp) -> Option<u32> {
        self.lookup_pk(external_id, ts).visible_id()
    }

    pub fn get_internal_id_by_i64(&self, external_id: i64, ts: Timestamp) -> Option<u32> {
        self.lookup_pk_by_i64(external_id, ts).visible_id()
    }

    /// Visibility-aware primary-key lookup with global ids. Read-locked:
    /// committed bindings invisible at `ts` resolve as missing, so callers
    /// need no secondary timestamp filtering.
    pub fn lookup_pk(&self, external_id: &str, ts: Timestamp) -> PkLookup {
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].read();
        match table.lookup_internal_id(&IdKey::Text(external_id.to_string()), ts) {
            PkLookup::Visible(local_id) => PkLookup::Visible(self.encode_id(idx, local_id)),
            PkLookup::Missing => PkLookup::Missing,
        }
    }

    /// Integer-keyed lookup. Same contract as [`lookup_pk`](Self::lookup_pk).
    pub fn lookup_pk_by_i64(&self, external_id: i64, ts: Timestamp) -> PkLookup {
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].read();
        match table.lookup_internal_id(&IdKey::Int(external_id), ts) {
            PkLookup::Visible(local_id) => PkLookup::Visible(self.encode_id(idx, local_id)),
            PkLookup::Missing => PkLookup::Missing,
        }
    }

    pub fn get_internal_id_raw(&self, external_id: &str) -> Option<u32> {
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].read();
        let local_id = table.get_internal_id_raw(external_id)?;
        Some(self.encode_id(idx, local_id))
    }

    pub fn get_internal_id_by_i64_raw(&self, external_id: i64) -> Option<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].read();
        let local_id = table.get_internal_id_by_i64_raw(external_id)?;
        Some(self.encode_id(idx, local_id))
    }

    pub fn get_external_id_raw(&self, global_id: u32) -> Option<IdKey> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.get_external_id_raw(local_id)
    }

    /// Total allocated vertex slots across all shards, including deleted but
    /// not yet reclaimed entries.
    ///
    /// Shards are read without a global lock, so concurrent inserts and
    /// deletes may be observed inconsistently across shards. Use it for sizing
    /// and statistics, not for exact live accounting. Exact live counts come
    /// from `approximate_id_hole_stats`. The `approximate_` prefix marks the
    /// cross-shard inconsistency in the name so callers cannot mistake it for
    /// a strongly consistent count.
    pub fn approximate_total_count(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.read().total_count();
        }
        total
    }

    /// Live vertex count at `ts` and total allocated local IDs across all
    /// shards.
    ///
    /// The difference (`allocated - live`) is the number of deleted-but-
    /// unreclaimed vertex slots. Edge CSR row space stays at the allocated
    /// high-water mark until compaction reclaims it, so a large gap is the
    /// trigger signal for automatic background compaction.
    ///
    /// Shards are read without a global lock, so the two numbers may come
    /// from different instants under concurrent writes. The `approximate_`
    /// prefix marks this cross-shard inconsistency; use the result for
    /// sizing and compaction signals, never as a strongly consistent census.
    pub fn approximate_id_hole_stats(&self, ts: Timestamp) -> (usize, usize) {
        let mut live = 0;
        let mut allocated = 0;
        for shard in &self.shards {
            let (l, a) = shard.read().id_hole_stats(ts);
            live += l;
            allocated += a;
        }
        (live, allocated)
    }

    /// External vertex-id keys of every live row, across all shards.
    ///
    /// Used to rebuild the self-proven vertex-id domain evidence after a
    /// restore (the write-path accumulator is not populated by disk loads).
    pub fn external_id_keys(&self) -> Vec<IdKey> {
        let mut keys = Vec::new();
        for shard in &self.shards {
            let table = shard.read();
            keys.extend(table.id_indexer.iter().into_iter().map(|(key, _)| key));
        }
        keys
    }
}

/// Pre-sized all-null column of the declared type for sharded merges, so
/// same-kind per-shard decodes scatter directly into a typed target.
fn empty_typed_column(data_type: Option<&DataType>, len: usize) -> ColumnValues {
    match data_type {
        Some(DataType::BigInt) => ColumnValues::I64 {
            values: vec![0; len],
            valid: vec![0; len],
        },
        Some(DataType::Double) => ColumnValues::F64 {
            values: vec![0.0; len],
            valid: vec![0; len],
        },
        Some(DataType::Int) => ColumnValues::I32 {
            values: vec![0; len],
            valid: vec![0; len],
        },
        Some(DataType::Bool) => ColumnValues::Bool {
            values: vec![0; len],
            valid: vec![0; len],
        },
        Some(DataType::SmallInt) => ColumnValues::I16 {
            values: vec![0; len],
            valid: vec![0; len],
        },
        Some(DataType::Float) => ColumnValues::F32 {
            values: vec![0.0; len],
            valid: vec![0; len],
        },
        _ => ColumnValues::General(vec![None; len]),
    }
}
