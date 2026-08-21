use std::path::Path;

use parking_lot::RwLock;

use super::core::{VertexTable, VertexTableConfig};
use crate::core::types::Timestamp;
use crate::core::{StorageResult, Value};
use crate::storage::compression::CompressionType;
use crate::storage::mvcc::SnapshotHandle;
use crate::storage::schema::ChangeDetails;
use crate::storage::types::StoragePropertyDef;
use crate::storage::vertex::{IdKey, VertexRecord};

/// Maximum shard count per vertex table. Lifted from 16 to 256 so a single
/// vertex label's write concurrency is no longer pinned to 16 on large
/// machines; the interleaved ID encoding supports any power-of-two count up
/// to this value within the u32 ID space (~16.7M vertices per shard).
const MAX_SHARDS: usize = 256;

/// Adapt the default shard count to the available CPU parallelism, clamped to
/// `MAX_SHARDS` and rounded down to a power of two (required by the shard-ID
/// bit encoding). Fallback to 1 if the platform cannot report parallelism.
fn default_num_shards() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_SHARDS)
        .next_power_of_two()
}
// Internal ID layout: `internal_id = (segment << K) | slot`.
// A shard's i-th segment is `shard + i * num_shards`, so segments are
// interleaved across shards and unique per shard, and the shard is recovered
// as `(id >> K) % num_shards` — pure bit arithmetic, no mapping table.
// Decoding the slot requires knowing which segment ordinal it belongs to:
// `local_id = (segment / num_shards) * 2^K + slot`, which keeps the
// shard-local ids dense (0..n) exactly as the underlying VertexTable rows.
//
// Because each shard's i-th segment is fixed at `shard + i * num_shards`,
// IDs stay proportional to the vertex count: with K = 12 and balanced shards,
// N vertices occupy IDs up to ~N + num_shards * 4096, versus the 16x blowup
// of the previous shard-in-low-bits encoding.
const SEGMENT_SLOTS_BITS: u32 = 12;
const SEGMENT_SLOTS: u32 = 1 << SEGMENT_SLOTS_BITS;
const SEGMENT_SLOTS_MASK: u32 = SEGMENT_SLOTS - 1;

fn encode_id(shard: usize, local_id: u32, num_shards: usize) -> u32 {
    debug_assert!(shard < num_shards);
    debug_assert!(local_id <= u32::MAX / num_shards as u32);
    let segment = shard as u32 + (local_id >> SEGMENT_SLOTS_BITS) * num_shards as u32;
    (segment << SEGMENT_SLOTS_BITS) | (local_id & SEGMENT_SLOTS_MASK)
}

fn decode_id(global_id: u32, num_shards: usize) -> (usize, u32) {
    let segment = global_id >> SEGMENT_SLOTS_BITS;
    let shard = (segment % num_shards as u32) as usize;
    let local_id =
        (segment / num_shards as u32) << SEGMENT_SLOTS_BITS | (global_id & SEGMENT_SLOTS_MASK);
    (shard, local_id)
}

pub struct ShardedVertexTable {
    shards: Vec<RwLock<VertexTable>>,
    num_shards: usize,
    label: crate::core::types::LabelId,
    label_name: String,
}

impl ShardedVertexTable {
    pub fn new(
        label: crate::core::types::LabelId,
        label_name: String,
        schema: crate::storage::vertex::VertexSchema,
    ) -> Self {
        Self::with_config(label, label_name, schema, default_num_shards())
    }

    pub fn with_config(
        label: crate::core::types::LabelId,
        label_name: String,
        schema: crate::storage::vertex::VertexSchema,
        num_shards: usize,
    ) -> Self {
        let num_shards = num_shards.clamp(1, MAX_SHARDS).next_power_of_two();
        let mut shards = Vec::with_capacity(num_shards);
        for _ in 0..num_shards {
            shards.push(RwLock::new(VertexTable::with_config(
                label,
                label_name.clone(),
                schema.clone(),
                VertexTableConfig::default(),
            )));
        }
        Self {
            shards,
            num_shards,
            label,
            label_name,
        }
    }

    fn shard_index_by_str(&self, external_id: &str) -> usize {
        let hash = fxhash(external_id);
        (hash as usize) & (self.num_shards - 1)
    }

    fn shard_index_by_i64(&self, external_id: i64) -> usize {
        let hash = fxhash_i64(external_id);
        (hash as usize) & (self.num_shards - 1)
    }

    #[cfg(test)]
    pub(crate) fn verify_invariants(&self) -> crate::core::StorageResult<()> {
        use crate::core::error::storage::StorageErrorKind;

        for shard in &self.shards {
            let table = shard.read();
            let id_count = table.id_indexer.len();

            for (key, idx) in table.id_indexer.iter() {
                let start_ts = table.timestamps.get_start_ts(idx);
                if start_ts.is_none() {
                    return Err(crate::core::StorageError::new(
                        StorageErrorKind::StorageError,
                        format!("ID {} for key {:?} missing in timestamps", idx, key),
                    ));
                }
            }

            for idx in 0..table.timestamps.size() {
                if let Some(_start_ts) = table.timestamps.get_start_ts(idx as u32) {
                    let key = table.id_indexer.get_key(idx as u32);
                    if key.is_none() {
                        return Err(crate::core::StorageError::new(
                            StorageErrorKind::StorageError,
                            format!("Timestamp entry {} missing in id_indexer", idx),
                        ));
                    }
                }
            }

            if table.columns.row_count() != id_count {
                return Err(crate::core::StorageError::new(
                    StorageErrorKind::StorageError,
                    format!(
                        "Column count ({}) mismatch with id_indexer.len() ({})",
                        table.columns.row_count(),
                        id_count
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Record an allocation of `local_id` in shard `idx` and encode it as a
    /// global id.
    fn record_allocation(&self, idx: usize, local_id: u32) -> u32 {
        encode_id(idx, local_id, self.num_shards)
    }

    fn decode_id(&self, global_id: u32) -> (usize, u32) {
        decode_id(global_id, self.num_shards)
    }

    fn encode_id(&self, shard: usize, local_id: u32) -> u32 {
        encode_id(shard, local_id, self.num_shards)
    }

    /// Zone-map pruning mask over `ids` (global internal ids).
    ///
    /// `mask[i] == false` means the row's zone-map chunk provably cannot
    /// contain values matching any of `ranges`, so the id can be skipped
    /// before decoding. Unknown columns, chunks without bounds, and
    /// non-scalar types keep the id (conservative).
    pub fn zone_prune_mask(
        &self,
        ids: &[u32],
        ranges: &[crate::storage::cursor::PredicateRange],
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
                let chunk = local_id as usize / crate::storage::vertex::column_store::ZONE_MAP_CHUNK_ROWS;
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

    // ==================== Write Operations ====================

    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_str(external_id);
        let mut table = self.shards[idx].write();
        let local_id = table.insert(external_id, properties, ts)?;
        Ok(self.record_allocation(idx, local_id))
    }

    pub fn insert_by_i64(
        &self,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let mut table = self.shards[idx].write();
        let local_id = table.insert_by_i64(external_id, properties, ts)?;
        Ok(self.record_allocation(idx, local_id))
    }

    pub fn delete(&self, external_id: &str, ts: Timestamp) -> StorageResult<()> {
        let idx = self.shard_index_by_str(external_id);
        let mut table = self.shards[idx].write();
        table.delete(external_id, ts)
    }

    pub fn delete_by_i64(&self, external_id: i64, ts: Timestamp) -> StorageResult<()> {
        let idx = self.shard_index_by_i64(external_id);
        let mut table = self.shards[idx].write();
        table.delete_by_i64(external_id, ts)
    }

    pub fn update_property(
        &self,
        global_id: u32,
        col_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].write();
        table.update_property(local_id, col_name, value, ts)
    }

    // ==================== Read Operations ====================

    pub fn get_by_internal_id(&self, global_id: u32, ts: Timestamp) -> Option<VertexRecord> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.get_by_internal_id(local_id, ts)
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
    pub fn external_id_keys(&self) -> Vec<crate::storage::vertex::IdKey> {
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

    // ==================== MVCC ====================

    pub fn register_snapshot(&self, ts: Timestamp) -> StorageResult<SnapshotHandle> {
        let handle = self.shards[0].write().register_snapshot(ts)?;
        for shard in &self.shards[1..] {
            shard.write().register_snapshot(ts)?;
        }
        Ok(handle)
    }

    pub fn unregister_snapshot(&self, handle: SnapshotHandle) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().unregister_snapshot(handle)?;
        }
        Ok(())
    }

    /// Unregister all snapshots with the given timestamp.
    /// Used by lazy registration cleanup on transaction finalize.
    pub fn unregister_snapshot_by_timestamp(&self, ts: Timestamp) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().unregister_snapshot_by_timestamp(ts)?;
        }
        Ok(())
    }

    /// Minimum timestamp among all active snapshots across shards
    /// (the GC watermark; `Timestamp::MAX` when no snapshot is active).
    /// Exposed for snapshot-lifecycle tests and diagnostics.
    #[cfg(test)]
    pub fn min_active_snapshot_ts(&self) -> Timestamp {
        self.shards
            .iter()
            .map(|shard| shard.read().min_active_snapshot_ts())
            .min()
            .unwrap_or(Timestamp::MAX)
    }

    pub fn gc(&self, min_ts: Timestamp) -> StorageResult<usize> {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.write().gc(min_ts)?;
        }
        Ok(total)
    }

    // ==================== Schema ====================

    pub fn schema(&self) -> crate::storage::vertex::VertexSchema {
        // Shard 0 is the schema authority: every shard holds the same schema
        // and all schema mutations are applied to each shard in order.
        self.shards[0].read().schema().clone()
    }

    pub fn apply_schema(&self, schema: crate::storage::vertex::VertexSchema) {
        for shard in &self.shards {
            shard.write().set_schema(schema.clone());
        }
    }

    pub fn label(&self) -> crate::core::types::LabelId {
        self.label
    }

    pub fn label_name(&self) -> &str {
        &self.label_name
    }

    pub fn memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for shard in &self.shards {
            total += shard.read().memory_size();
        }
        total
    }

    pub fn num_shards(&self) -> usize {
        self.num_shards
    }

    // ==================== Additional Read Operations ====================

    pub fn get_projected_by_internal_id(
        &self,
        global_id: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<VertexRecord> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].read();
        table.get_projected_by_internal_id(local_id, ts, projection)
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

    // ==================== Additional Write Operations ====================

    pub fn delete_by_internal_id(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].write();
        table.delete_by_internal_id(local_id, ts)
    }

    pub fn revert_delete(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].write();
        table.revert_delete(local_id, ts)
    }

    pub fn update_property_by_id(
        &self,
        global_id: u32,
        col_id: i32,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].write();
        table.update_property_by_id(local_id, col_id, value, ts)
    }

    pub fn batch_delete(&self, external_ids: &[&str], ts: Timestamp) -> StorageResult<usize> {
        // Route ids to their owning shard and delete each shard's batch under
        // a single lock, instead of locking per id.
        let mut by_shard: Vec<Vec<&str>> = vec![Vec::new(); self.num_shards];
        for id in external_ids {
            by_shard[self.shard_index_by_str(id)].push(id);
        }
        let mut total = 0;
        for (idx, ids) in by_shard.iter().enumerate() {
            if !ids.is_empty() {
                total += self.shards[idx].write().batch_delete(ids, ts)?;
            }
        }
        Ok(total)
    }

    pub fn batch_delete_i64(&self, external_ids: &[i64], ts: Timestamp) -> StorageResult<usize> {
        let mut by_shard: Vec<Vec<i64>> = vec![Vec::new(); self.num_shards];
        for id in external_ids {
            by_shard[self.shard_index_by_i64(*id)].push(*id);
        }
        let mut total = 0;
        for (idx, ids) in by_shard.iter().enumerate() {
            if !ids.is_empty() {
                total += self.shards[idx].write().batch_delete_i64(ids, ts)?;
            }
        }
        Ok(total)
    }

    pub fn reserve_id_capacity(&self, additional: usize) {
        for shard in &self.shards {
            shard.write().reserve_id_capacity(additional);
        }
    }

    /// Resolve the external vertex IDs of `global_ids` that are valid at `ts`
    /// (A1).  Aligned with the input; invalid ids yield `None`.
    pub fn resolve_valid_ids(
        &self,
        global_ids: &[u32],
        ts: Timestamp,
    ) -> Vec<Option<crate::core::types::VertexId>> {
        let mut groups: Vec<Vec<(usize, u32)>> = vec![Vec::new(); self.num_shards];
        for (out_idx, &global_id) in global_ids.iter().enumerate() {
            let (shard_idx, local_id) = self.decode_id(global_id);
            groups[shard_idx].push((out_idx, local_id));
        }
        let mut out: Vec<Option<crate::core::types::VertexId>> =
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
    ) -> Vec<(String, crate::storage::cursor::ColumnValues)> {
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
        let types: Vec<Option<crate::core::types::DataType>> = {
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

        let mut merged: Vec<(String, crate::storage::cursor::ColumnValues)> = resolved_names
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    crate::storage::cursor::ColumnValues::General(vec![None; global_ids.len()]),
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
                if let Some(typed) = crate::storage::cursor::ColumnValues::from_general_with_type(
                    general, &data_type,
                ) {
                    merged[index].1 = typed;
                }
            }
        }
        merged
    }

    pub fn active_snapshot_count(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.read().active_snapshot_count();
        }
        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for shard in &self.shards {
            total += shard.read().used_memory_size();
        }
        total
    }

    // ==================== Schema Operations ====================

    pub fn add_property(&self, prop: StoragePropertyDef) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().add_property(prop.clone())?;
        }
        Ok(())
    }

    pub fn remove_property(&self, prop_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().remove_property(prop_name)?;
        }
        Ok(())
    }

    pub fn rename_property(&self, old_name: &str, new_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            shard.write().rename_property(old_name, new_name)?;
        }
        Ok(())
    }

    pub fn rebuild_schema_change_from_redo(&self, details: ChangeDetails) -> StorageResult<()> {
        for shard in &self.shards {
            shard
                .write()
                .rebuild_schema_change_from_redo(details.clone())?;
        }
        Ok(())
    }

    // ==================== Compaction ====================

    /// Compact vertices deleted at or before `ts` across all shards.
    ///
    /// Returns the removed external keys and the old-to-new *global* internal
    /// ID mapping (shard-local rows translated into encoded global IDs), which
    /// callers must propagate to edge CSR rows before dependent queries.
    pub fn compact_with_ts_collect_mapping(
        &self,
        ts: Timestamp,
    ) -> StorageResult<(Vec<IdKey>, std::collections::HashMap<u32, u32>)> {
        let mut all_removed = Vec::new();
        let mut all_mapping = std::collections::HashMap::new();
        for (idx, shard) in self.shards.iter().enumerate() {
            let mut table = shard.write();
            let (removed, local_mapping) = table.compact_with_ts_collect_mapping(ts)?;
            for (old_local, new_local) in local_mapping {
                all_mapping.insert(
                    self.encode_id(idx, old_local),
                    self.encode_id(idx, new_local),
                );
            }
            all_removed.extend(removed);
        }
        Ok((all_removed, all_mapping))
    }

    // ==================== Version History ====================

    pub fn version_history_ref(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<crate::storage::schema::LabelVersionHistory>> {
        self.shards[0].read().version_history_ref()
    }

    // ==================== Persistence ====================

    pub fn flush<P: AsRef<Path>>(
        &self,
        path: P,
        compression: CompressionType,
    ) -> StorageResult<()> {
        use std::fs;
        let path = path.as_ref();
        fs::create_dir_all(path)?;
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = path.join(format!("shard_{}", i));
            shard.write().flush(&shard_dir, compression)?;
        }
        Ok(())
    }

    pub fn load<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = path.join(format!("shard_{}", i));
            if shard_dir.exists() {
                let mut table = shard.write();
                table.load(&shard_dir)?;
            }
        }
        Ok(())
    }
}

fn fxhash(s: &str) -> u64 {
    let mut hash: u64 = 0;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(0x517cc1b727220a95);
        hash ^= byte as u64;
    }
    hash
}

fn fxhash_i64(n: i64) -> u64 {
    let mut hash: u64 = 0x517cc1b727220a95;
    hash ^= n as u64;
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::MAX_TIMESTAMP;
    const TEST_TS: Timestamp = MAX_TIMESTAMP - 1;
    use crate::core::{DataType, Value};
    use crate::storage::types::StoragePropertyDef;
    use std::sync::Arc;

    fn test_schema() -> crate::storage::vertex::VertexSchema {
        crate::storage::vertex::VertexSchema {
            label_id: 1,
            label_name: "person".to_string(),
            properties: vec![
                StoragePropertyDef::new("name".to_string(), DataType::String),
                StoragePropertyDef {
                    name: "age".to_string(),
                    data_type: DataType::Int,
                    nullable: true,
                    default_value: None,
                },
            ],
            primary_key_index: 0,
            schema_version: 1,
        }
    }

    #[test]
    fn test_encode_decode_id() {
        for num_shards in [1usize, 2, 4, 8, 16, 32, 64, 128, 256] {
            for shard in 0..num_shards {
                for local in [
                    0,
                    1,
                    42,
                    SEGMENT_SLOTS - 1,
                    SEGMENT_SLOTS,
                    SEGMENT_SLOTS * num_shards as u32,
                    u32::MAX / num_shards as u32,
                ] {
                    let e = encode_id(shard, local, num_shards);
                    let (s, l) = decode_id(e, num_shards);
                    assert_eq!(s, shard, "shard mismatch: {e:#x}");
                    assert_eq!(l, local, "local mismatch: {e:#x}");
                }
            }
        }
    }

    #[test]
    fn test_segment_allocation_spans_boundaries() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts = TEST_TS;
        let mut ids = Vec::new();
        let n = SEGMENT_SLOTS as usize + 32;
        for i in 0..n {
            let id = insert_with_name(&table, &format!("s_{}", i), ts);
            ids.push(id);
        }
        for (i, &id) in ids.iter().enumerate() {
            let record = table.get_by_internal_id(id, ts).unwrap();
            assert_eq!(
                record
                    .properties
                    .iter()
                    .find(|(k, _)| k == "name")
                    .unwrap()
                    .1,
                Value::from(format!("s_{}", i))
            );
        }
        assert_eq!(table.total_count(), n);
    }

    #[test]
    fn test_load_resumes_allocation() {
        let dir = std::env::temp_dir().join(format!("sharded_load_{}", std::process::id()));
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        for i in 0..SEGMENT_SLOTS + 100 {
            insert_with_name(&table, &format!("v_{}", i), ts);
        }
        table
            .flush(
                &dir,
                crate::storage::compression::CompressionType::Zstd { level: 0 },
            )
            .unwrap();

        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        reloaded.load(&dir).unwrap();

        for i in 0..SEGMENT_SLOTS + 100 {
            assert!(reloaded.get_internal_id(&format!("v_{}", i), ts).is_some());
        }

        let new_id = insert_with_name(&reloaded, "v_new_after_load", ts);
        let new_id2 = insert_with_name(&reloaded, "v_new_after_load2", ts);
        assert_ne!(new_id, new_id2);
        assert!(reloaded.get_by_internal_id(new_id, ts).is_some());
        assert!(reloaded.get_internal_id("v_new_after_load", ts).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_insert_and_read() {
        let table = ShardedVertexTable::with_config(1, "person".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        let id = table
            .insert(
                "alice",
                &[
                    ("name".to_string(), Value::from("Alice")),
                    ("age".to_string(), Value::from(30i64)),
                ],
                ts,
            )
            .unwrap();
        let record = table.get_by_internal_id(id, ts).unwrap();
        assert_eq!(record.properties.len(), 2);
    }

    fn insert_with_name(table: &ShardedVertexTable, name: &str, ts: Timestamp) -> u32 {
        table
            .insert(name, &[("name".to_string(), Value::from(name))], ts)
            .unwrap()
    }

    #[test]
    fn test_delete() {
        let table = ShardedVertexTable::with_config(1, "person".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        let id = insert_with_name(&table, "bob", ts);
        assert!(table.get_by_internal_id(id, ts).is_some());
        table.delete("bob", ts).unwrap();
        assert!(table.get_by_internal_id(id, ts).is_none());
    }

    #[test]
    fn test_get_internal_id_roundtrip() {
        let table = ShardedVertexTable::with_config(1, "test".to_string(), test_schema(), 8);
        let ts = TEST_TS;
        let id = insert_with_name(&table, "charlie", ts);
        let found = table.get_internal_id("charlie", ts).unwrap();
        assert_eq!(id, found);
    }

    #[test]
    fn test_concurrent_inserts() {
        let table = Arc::new(ShardedVertexTable::with_config(
            1,
            "person".to_string(),
            test_schema(),
            8,
        ));
        let ts = TEST_TS;
        let t1 = Arc::clone(&table);
        let t2 = Arc::clone(&table);
        let h1 = std::thread::spawn(move || {
            for i in 0..100 {
                t1.insert(
                    &format!("user_{}", i),
                    &[("name".to_string(), Value::from(format!("user_{}", i)))],
                    ts,
                )
                .unwrap();
            }
        });
        let h2 = std::thread::spawn(move || {
            for i in 100..200 {
                t2.insert(
                    &format!("user_{}", i),
                    &[("name".to_string(), Value::from(format!("user_{}", i)))],
                    ts,
                )
                .unwrap();
            }
        });
        h1.join().unwrap();
        h2.join().unwrap();
        assert_eq!(table.total_count(), 200);
    }

    #[test]
    fn test_scan() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        for i in 0..50 {
            insert_with_name(&table, &format!("k{}", i), ts);
        }
        let results = table.scan(ts);
        assert_eq!(results.len(), 50);
    }

    #[test]
    fn test_gc() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts1 = 200;
        let ts2 = 100;
        insert_with_name(&table, "gc_test", ts1);
        table.delete("gc_test", ts2).unwrap();
        let count = table.gc(150).unwrap();
        assert!(count > 0);
    }

    #[test]
    fn test_id_uniqueness_across_shards() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 16);
        let ts = TEST_TS;
        let mut ids = std::collections::HashSet::new();
        for i in 0..200 {
            let id = insert_with_name(&table, &format!("unique_{}", i), ts);
            assert!(ids.insert(id), "duplicate internal_id: {}", id);
        }
        assert_eq!(ids.len(), 200);
    }

    #[test]
    fn test_id_hole_stats_tracks_allocated_and_live() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 8);
        let ts_insert = 200;
        let ts_delete = 100;
        for i in 0..100 {
            insert_with_name(&table, &format!("v_{}", i), ts_insert);
        }
        let (live, allocated) = table.id_hole_stats(150);
        assert_eq!((live, allocated), (100, 100));

        for i in 0..30 {
            table.delete(&format!("v_{}", i), ts_delete).unwrap();
        }
        // Deleted vertices leave holes: allocated stays at the high-water
        // mark, live only counts vertices not deleted at the cutoff.
        let (live, allocated) = table.id_hole_stats(150);
        assert_eq!((live, allocated), (70, 100));
        // A cutoff before the deletes sees no holes.
        let (live, allocated) = table.id_hole_stats(50);
        assert_eq!((live, allocated), (100, 100));

        // Physical removal + compaction re-densifies local IDs and resets
        // the allocation counters (same path as compact_vertex_remap).
        let (removed, mapping) = table.compact_with_ts_collect_mapping(ts_insert).unwrap();
        assert_eq!(removed.len(), 30);
        assert!(!mapping.is_empty());

        let (live, allocated) = table.id_hole_stats(150);
        assert_eq!((live, allocated), (70, 70));
    }

    #[test]
    fn test_internal_id_upper_bound_across_shard_counts() {
        for num_shards in [1usize, 2, 4, 8, 32, 128, 256] {
            let table =
                ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), num_shards);
            let ts = TEST_TS;
            let n = 20_000;
            for i in 0..n {
                insert_with_name(&table, &format!("v_{}_{}", num_shards, i), ts);
            }
            let max_id = (0..n)
                .filter_map(|i| table.get_internal_id(&format!("v_{}_{}", num_shards, i), ts))
                .max()
                .unwrap();
            assert!(
                (max_id as usize) <= num_shards * n,
                "num_shards={}: max_id {max_id} exceeds upper bound {}",
                num_shards,
                num_shards * n
            );
        }
    }
}
