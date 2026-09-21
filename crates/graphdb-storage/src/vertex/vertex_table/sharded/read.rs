use super::routing::decode_id;
use super::ShardedVertexTable;
use crate::vertex::{IdKey, VertexRecord};
use graphdb_core::types::Timestamp;

impl ShardedVertexTable {
    /// Zone-map pruning mask over `ids` (global internal ids).
    ///
    /// `mask[i] == false` means the row's zone-map chunk provably cannot
    /// contain values matching any of `ranges`, so the id can be skipped
    /// before decoding. Unknown columns, chunks without bounds, and
    /// non-scalar types keep the id (conservative).
    pub fn zone_prune_mask(
        &self,
        ids: &[u32],
        ranges: &[crate::cursor::PredicateRange],
    ) -> Vec<bool> {
        let mut mask = vec![true; ids.len()];
        if ranges.is_empty() {
            return mask;
        }
        // Group positions by shard so each shard is locked once per batch.
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.num_shards];
        for (pos, &id) in ids.iter().enumerate() {
            let (shard, _) = decode_id(id, self.num_shards);
            by_shard[shard].push(pos);
        }
        for (shard_idx, positions) in by_shard.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let table = self.shards[shard_idx].read();
            for &pos in positions {
                let (_, local_id) = decode_id(ids[pos], self.num_shards);
                let chunk = local_id as usize / crate::vertex::column_store::ZONE_MAP_CHUNK_ROWS;
                for range in ranges {
                    let Some(bounds) = table.columns.zone_maps_for_column(&range.column) else {
                        continue;
                    };
                    let Some(zb) = bounds.get(chunk) else {
                        continue;
                    };
                    let (Some(min), Some(max)) = (&zb.min, &zb.max) else {
                        continue;
                    };
                    if !range.overlaps(min, max) {
                        mask[pos] = false;
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
    pub fn column_stats_snapshot(
        &self,
        column: &str,
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
            row_count: self.total_count() as u64,
            null_count,
            distinct_count,
            hll,
            min_value: min,
            max_value: max,
        })
    }

    pub fn get_by_internal_id(&self, global_id: u32, ts: Timestamp) -> Option<VertexRecord> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.get_by_internal_id(local_id, ts).map(|mut record| {
            record.internal_id = global_id;
            record
        })
    }

    /// Row survival stamps for pending-aware rechecks (shard-decoded).
    pub fn row_timestamps(&self, global_id: u32) -> Option<(Timestamp, Option<Timestamp>)> {
        let (idx, local_id) = self.decode_id(global_id);
        self.shards[idx].read().row_timestamps(local_id)
    }

    /// Per-column covering version stamps for pending-aware rechecks.
    pub fn row_picked_starts(&self, global_id: u32, ts: Timestamp) -> Vec<Timestamp> {
        let (idx, local_id) = self.decode_id(global_id);
        self.shards[idx].read().row_picked_starts(local_id, ts)
    }

    pub fn get_internal_id(&self, external_id: &str, ts: Timestamp) -> Option<u32> {
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].read();
        let local_id = table.get_internal_id(external_id, ts)?;
        Some(self.encode_id(idx, local_id))
    }

    pub fn get_internal_id_by_i64(&self, external_id: i64, ts: Timestamp) -> Option<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].read();
        let local_id = table.get_internal_id_by_i64(external_id, ts)?;
        Some(self.encode_id(idx, local_id))
    }

    /// Total allocated vertex slots across all shards.
    ///
    /// This is an approximate live count: shards are read without a global
    /// lock, so concurrent inserts/deletes may be observed inconsistently
    /// across shards. Use it for sizing and statistics, not for exact
    /// accounting.
    pub fn total_count(&self) -> usize {
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
    pub fn id_hole_stats(&self, ts: Timestamp) -> (usize, usize) {
        let mut live = 0;
        let mut allocated = 0;
        for shard in &self.shards {
            let (l, a) = shard.read().id_hole_stats(ts);
            live += l;
            allocated += a;
        }
        (live, allocated)
    }

    pub fn scan(&self, ts: Timestamp) -> Vec<VertexRecord> {
        use rayon::prelude::*;
        let per_shard: Vec<(usize, Vec<VertexRecord>)> = self
            .shards
            .par_iter()
            .enumerate()
            .map(|(shard_idx, shard)| {
                let table = shard.read();
                let records: Vec<VertexRecord> = table
                    .scan(ts)
                    .map(|mut record| {
                        record.internal_id = self.encode_id(shard_idx, record.internal_id);
                        record
                    })
                    .collect();
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

    /// Live global internal IDs (shard-encoded), in shard order.
    ///
    /// Mirrors the ordering of the previous `scan_projected` so lazy
    /// paginated scans yield records in a stable order.
    pub fn live_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for (shard_idx, shard) in self.shards.iter().enumerate() {
            let table = shard.read();
            ids.extend(
                table
                    .live_ids()
                    .into_iter()
                    .map(|local_id| self.encode_id(shard_idx, local_id)),
            );
        }
        ids
    }

    /// External vertex-id keys of every live row, across all shards.
    ///
    /// Used to rebuild the self-proven vertex-id domain evidence after a
    /// restore (the write-path accumulator is not populated by disk loads).
    pub fn external_id_keys(&self) -> Vec<crate::vertex::IdKey> {
        let mut keys = Vec::new();
        for shard in &self.shards {
            let table = shard.read();
            keys.extend(table.id_indexer.iter().into_iter().map(|(key, _)| key));
        }
        keys
    }

    /// Batch variant of [`get_projected_by_internal_id`].
    ///
    /// Input ids are grouped by shard, decoded with one lock acquisition and
    /// one batch call per shard, then re-encoded to global ids. The output is
    /// aligned with the input order; invalid ids yield `None`.
    pub fn get_projected_batch(
        &self,
        global_ids: &[u32],
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<Option<VertexRecord>> {
        let mut groups: Vec<Vec<(usize, u32)>> = vec![Vec::new(); self.num_shards];
        for (out_idx, &global_id) in global_ids.iter().enumerate() {
            let (shard_idx, local_id) = self.decode_id(global_id);
            groups[shard_idx].push((out_idx, local_id));
        }

        let mut out: Vec<Option<VertexRecord>> = global_ids.iter().map(|_| None).collect();
        for (shard_idx, group) in groups.into_iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            let locals: Vec<u32> = group.iter().map(|&(_, local)| local).collect();
            let table = self.shards[shard_idx].read();
            let records = table.get_projected_batch(&locals, ts, projection);
            for ((out_idx, _), record) in group.into_iter().zip(records) {
                out[out_idx] = record.map(|mut rec| {
                    rec.internal_id = self.encode_id(shard_idx, rec.internal_id);
                    rec
                });
            }
        }
        out
    }

    pub fn get_projected_by_internal_id(
        &self,
        global_id: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<VertexRecord> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table
            .get_projected_by_internal_id(local_id, ts, projection)
            .map(|mut record| {
                record.internal_id = global_id;
                record
            })
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

    pub fn get_external_id(&self, global_id: u32, ts: Timestamp) -> Option<IdKey> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.get_external_id(local_id, ts)
    }

    pub fn get_external_id_raw(&self, global_id: u32) -> Option<IdKey> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.get_external_id_raw(local_id)
    }

    /// Resolve the external vertex IDs of `global_ids` that are valid at `ts`
    /// (A1).  Aligned with the input; invalid ids yield `None`.
    pub fn resolve_valid_ids(
        &self,
        global_ids: &[u32],
        ts: Timestamp,
    ) -> Vec<Option<graphdb_core::types::VertexId>> {
        let mut groups: Vec<Vec<(usize, u32)>> = vec![Vec::new(); self.num_shards];
        for (out_idx, &global_id) in global_ids.iter().enumerate() {
            let (shard_idx, local_id) = self.decode_id(global_id);
            groups[shard_idx].push((out_idx, local_id));
        }
        let mut out: Vec<Option<graphdb_core::types::VertexId>> =
            global_ids.iter().map(|_| None).collect();
        for (shard_idx, group) in groups.into_iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            let locals: Vec<u32> = group.iter().map(|&(_, local)| local).collect();
            let table = self.shards[shard_idx].read();
            let resolved = table.resolve_valid_ids(&locals, ts);
            for ((out_idx, _), vid) in group.into_iter().zip(resolved) {
                out[out_idx] = vid;
            }
        }
        out
    }

    /// Column-major batch decode (A1).  Input global ids are grouped by shard,
    /// decoded column-at-a-time per shard, then merged back into input order.
    /// When `names` is empty every column of the table is decoded.
    pub fn get_projected_columns(
        &self,
        global_ids: &[u32],
        ts: Timestamp,
        names: &[String],
    ) -> Vec<(String, crate::cursor::ColumnValues)> {
        let resolved_names: Vec<String> = if names.is_empty() {
            let table = self.shards[0].read();
            table
                .schema()
                .properties
                .iter()
                .map(|p| p.name.clone())
                .collect()
        } else {
            names.to_vec()
        };
        let types: Vec<Option<graphdb_core::types::DataType>> = {
            let table = self.shards[0].read();
            resolved_names
                .iter()
                .map(|n| table.data_type_of(n))
                .collect()
        };

        let mut groups: Vec<Vec<(usize, u32)>> = vec![Vec::new(); self.num_shards];
        for (out_idx, &global_id) in global_ids.iter().enumerate() {
            let (shard_idx, local_id) = self.decode_id(global_id);
            groups[shard_idx].push((out_idx, local_id));
        }

        let mut merged: Vec<(String, crate::cursor::ColumnValues)> = resolved_names
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    crate::cursor::ColumnValues::General(vec![None; global_ids.len()]),
                )
            })
            .collect();

        for (shard_idx, group) in groups.into_iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            let locals: Vec<u32> = group.iter().map(|&(_, local)| local).collect();
            let table = self.shards[shard_idx].read();
            let decoded = table.get_projected_columns(&locals, ts, &resolved_names);
            for (name, column) in decoded {
                if let Some((_, target)) = merged.iter_mut().find(|(n, _)| n == &name) {
                    column.scatter(target, &group);
                }
            }
        }

        for (index, data_type) in types.into_iter().enumerate() {
            if let Some(data_type) = data_type {
                let general = merged[index].1.to_general();
                if let Some(typed) =
                    crate::cursor::ColumnValues::from_general_with_type(general, &data_type)
                {
                    merged[index].1 = typed;
                }
            }
        }
        merged
    }
}
