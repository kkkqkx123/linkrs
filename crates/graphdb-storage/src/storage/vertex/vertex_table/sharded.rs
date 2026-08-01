use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;

use super::core::{VertexTable, VertexTableConfig};
use crate::core::types::Timestamp;
use crate::core::{StorageResult, Value};
use crate::storage::compression::CompressionType;
use crate::storage::mvcc::SnapshotHandle;
use crate::storage::schema::ChangeDetails;
use crate::storage::types::StoragePropertyDef;
use crate::storage::vertex::{IdKey, VertexRecord};

const DEFAULT_NUM_SHARDS: usize = 8;
const MAX_SHARDS: usize = 16;

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

struct Shard {
    table: Mutex<VertexTable>,
    /// Next local id to allocate in this shard (dense 0..n, never reused).
    /// Only modified while holding `table`'s lock.
    local_counter: AtomicU32,
    /// Segment currently being filled: `shard + (local_counter >> K) * num_shards`.
    /// Only modified while holding `table`'s lock.
    current_segment: AtomicU32,
}

pub struct ShardedVertexTable {
    shards: Vec<Shard>,
    num_shards: usize,
    /// Global segment allocator, advanced once per segment exhaustion.
    ///
    /// Segments are issued in round-robin order (segment `c` belongs to
    /// shard `c % num_shards`), but out-of-order claims can consume another
    /// shard's residue class, so the counter cannot be the source of truth
    /// for segment values. The segment a shard actually fills is always
    /// derived deterministically (`shard + ordinal * num_shards`), which is
    /// exactly what the round-robin order yields when claims stay in sync.
    segment_allocator: AtomicU32,
    label: crate::core::types::LabelId,
    label_name: String,
}

impl ShardedVertexTable {
    pub fn new(
        label: crate::core::types::LabelId,
        label_name: String,
        schema: crate::storage::vertex::VertexSchema,
    ) -> Self {
        Self::with_config(label, label_name, schema, DEFAULT_NUM_SHARDS)
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
            shards.push(Shard {
                table: Mutex::new(VertexTable::with_config(
                    label,
                    label_name.clone(),
                    schema.clone(),
                    VertexTableConfig::default(),
                )),
                local_counter: AtomicU32::new(0),
                current_segment: AtomicU32::new(0),
            });
        }
        Self {
            shards,
            num_shards,
            segment_allocator: AtomicU32::new(0),
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

    /// Segment that covers `local_id` in shard `idx`.
    fn segment_of(&self, idx: usize, local_id: u32) -> u32 {
        idx as u32 + (local_id >> SEGMENT_SLOTS_BITS) * self.num_shards as u32
    }

    /// Advance the global segment allocator by one.
    fn claim_segment(&self) {
        self.segment_allocator.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an allocation of `local_id` in shard `idx` and encode it as a
    /// global id. Updates the per-shard counter/segment and claims a fresh
    /// segment from the global allocator when the current one is exhausted.
    fn record_allocation(&self, idx: usize, shard: &Shard, local_id: u32) -> u32 {
        let prev_counter = shard.local_counter.load(Ordering::Relaxed);
        let new_counter = prev_counter.max(local_id + 1);
        let new_segment = self.segment_of(idx, new_counter);
        if new_segment != shard.current_segment.load(Ordering::Relaxed) {
            self.claim_segment();
        }
        shard.local_counter.store(new_counter, Ordering::Relaxed);
        shard.current_segment.store(new_segment, Ordering::Relaxed);
        encode_id(idx, local_id, self.num_shards)
    }

    fn decode_id(&self, global_id: u32) -> (usize, u32) {
        decode_id(global_id, self.num_shards)
    }

    fn encode_id(&self, shard: usize, local_id: u32) -> u32 {
        encode_id(shard, local_id, self.num_shards)
    }

    // ==================== Write Operations ====================

    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_str(external_id);
        let shard = &self.shards[idx];
        let mut table = shard.table.lock();
        let local_id = table.insert(external_id, properties, ts)?;
        Ok(self.record_allocation(idx, shard, local_id))
    }

    pub fn insert_by_i64(
        &self,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let shard = &self.shards[idx];
        let mut table = shard.table.lock();
        let local_id = table.insert_by_i64(external_id, properties, ts)?;
        Ok(self.record_allocation(idx, shard, local_id))
    }

    pub fn delete(&self, external_id: &str, ts: Timestamp) -> StorageResult<()> {
        let idx = self.shard_index_by_str(external_id);
        let mut table = self.shards[idx].table.lock();
        table.delete(external_id, ts)
    }

    pub fn delete_by_i64(&self, external_id: i64, ts: Timestamp) -> StorageResult<()> {
        let idx = self.shard_index_by_i64(external_id);
        let mut table = self.shards[idx].table.lock();
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
        let mut table = self.shards[idx].table.lock();
        table.update_property(local_id, col_name, value, ts)
    }

    // ==================== Read Operations ====================

    pub fn get_by_internal_id(&self, global_id: u32, ts: Timestamp) -> Option<VertexRecord> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].table.lock();
        table.get_by_internal_id(local_id, ts)
    }

    pub fn get_internal_id(&self, external_id: &str, ts: Timestamp) -> Option<u32> {
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].table.lock();
        let local_id = table.get_internal_id(external_id, ts)?;
        Some(self.encode_id(idx, local_id))
    }

    pub fn get_internal_id_by_i64(&self, external_id: i64, ts: Timestamp) -> Option<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].table.lock();
        let local_id = table.get_internal_id_by_i64(external_id, ts)?;
        Some(self.encode_id(idx, local_id))
    }

    pub fn total_count(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.table.lock().total_count();
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
            let (l, a) = shard.table.lock().id_hole_stats(ts);
            live += l;
            allocated += a;
        }
        (live, allocated)
    }

    pub fn scan(&self, ts: Timestamp) -> Vec<VertexRecord> {
        let mut all = Vec::new();
        for (shard_idx, shard) in self.shards.iter().enumerate() {
            let table = shard.table.lock();
            for mut record in table.scan(ts) {
                record.internal_id = self.encode_id(shard_idx, record.internal_id);
                all.push(record);
            }
        }
        all
    }

    /// Live global internal IDs (shard-encoded), in shard order.
    ///
    /// Mirrors the ordering of the previous `scan_projected` so lazy
    /// paginated scans yield records in a stable order.
    pub fn live_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for (shard_idx, shard) in self.shards.iter().enumerate() {
            let table = shard.table.lock();
            ids.extend(
                table
                    .live_ids()
                    .into_iter()
                    .map(|local_id| self.encode_id(shard_idx, local_id)),
            );
        }
        ids
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
            let table = self.shards[shard_idx].table.lock();
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
        let handle = self.shards[0].table.lock().register_snapshot(ts)?;
        for shard in &self.shards[1..] {
            shard.table.lock().register_snapshot(ts)?;
        }
        Ok(handle)
    }

    pub fn unregister_snapshot(&self, handle: SnapshotHandle) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().unregister_snapshot(handle)?;
        }
        Ok(())
    }

    pub fn gc(&self, min_ts: Timestamp) -> StorageResult<usize> {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.table.lock().gc(min_ts)?;
        }
        Ok(total)
    }

    // ==================== Schema ====================

    pub fn schema(&self) -> crate::storage::vertex::VertexSchema {
        // Shard 0 is the schema authority: every shard holds the same schema
        // and all schema mutations are applied to each shard in order.
        self.shards[0].table.lock().schema().clone()
    }

    pub fn apply_schema(&self, schema: crate::storage::vertex::VertexSchema) {
        for shard in &self.shards {
            shard.table.lock().set_schema(schema.clone());
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
            total += shard.table.lock().memory_size();
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
        let table = self.shards[idx].table.lock();
        table.get_projected_by_internal_id(local_id, ts, projection)
    }

    pub fn get_internal_id_raw(&self, external_id: &str) -> Option<u32> {
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].table.lock();
        let local_id = table.get_internal_id_raw(external_id)?;
        Some(self.encode_id(idx, local_id))
    }

    pub fn get_internal_id_by_i64_raw(&self, external_id: i64) -> Option<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].table.lock();
        let local_id = table.get_internal_id_by_i64_raw(external_id)?;
        Some(self.encode_id(idx, local_id))
    }

    pub fn get_external_id(&self, global_id: u32, ts: Timestamp) -> Option<IdKey> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].table.lock();
        table.get_external_id(local_id, ts)
    }

    pub fn get_external_id_raw(&self, global_id: u32) -> Option<IdKey> {
        let (idx, local_id) = self.decode_id(global_id);
        let table = self.shards[idx].table.lock();
        table.get_external_id_raw(local_id)
    }

    // ==================== Additional Write Operations ====================

    pub fn delete_by_internal_id(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].table.lock();
        table.delete_by_internal_id(local_id, ts)
    }

    pub fn revert_delete(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].table.lock();
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
        let mut table = self.shards[idx].table.lock();
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
                total += self.shards[idx].table.lock().batch_delete(ids, ts)?;
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
                total += self.shards[idx].table.lock().batch_delete_i64(ids, ts)?;
            }
        }
        Ok(total)
    }

    pub fn reserve_id_capacity(&self, additional: usize) {
        for shard in &self.shards {
            shard.table.lock().reserve_id_capacity(additional);
        }
    }

    pub fn active_snapshot_count(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.table.lock().active_snapshot_count();
        }
        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for shard in &self.shards {
            total += shard.table.lock().used_memory_size();
        }
        total
    }

    // ==================== Schema Operations ====================

    pub fn add_property(&self, prop: StoragePropertyDef) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().add_property(prop.clone())?;
        }
        Ok(())
    }

    pub fn remove_property(&self, prop_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().remove_property(prop_name)?;
        }
        Ok(())
    }

    pub fn rename_property(&self, old_name: &str, new_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().rename_property(old_name, new_name)?;
        }
        Ok(())
    }

    pub fn rebuild_schema_change_from_redo(&self, details: ChangeDetails) -> StorageResult<()> {
        for shard in &self.shards {
            shard
                .table
                .lock()
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
    ///
    /// Shard allocation counters are refreshed so the next allocation resumes
    /// from the compacted density instead of the pre-compaction high-water mark.
    pub fn compact_with_ts_collect_mapping(
        &self,
        ts: Timestamp,
    ) -> StorageResult<(Vec<IdKey>, std::collections::HashMap<u32, u32>)> {
        let mut all_removed = Vec::new();
        let mut all_mapping = std::collections::HashMap::new();
        for (idx, shard) in self.shards.iter().enumerate() {
            let mut table = shard.table.lock();
            let (removed, local_mapping) = table.compact_with_ts_collect_mapping(ts)?;
            for (old_local, new_local) in local_mapping {
                all_mapping.insert(
                    self.encode_id(idx, old_local),
                    self.encode_id(idx, new_local),
                );
            }
            all_removed.extend(removed);
            let next_local = table.next_local_id();
            shard.local_counter.store(next_local, Ordering::Relaxed);
            shard
                .current_segment
                .store(self.segment_of(idx, next_local), Ordering::Relaxed);
        }
        Ok((all_removed, all_mapping))
    }

    // ==================== Version History ====================

    pub fn version_history_ref(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<crate::storage::schema::LabelVersionHistory>> {
        self.shards[0].table.lock().version_history_ref()
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
            shard.table.lock().flush(&shard_dir, compression)?;
        }
        Ok(())
    }

    pub fn load<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        for (i, shard) in self.shards.iter().enumerate() {
            let shard_dir = path.join(format!("shard_{}", i));
            if shard_dir.exists() {
                let mut table = shard.table.lock();
                table.load(&shard_dir)?;
                let next_local = table.next_local_id();
                shard.local_counter.store(next_local, Ordering::Relaxed);
                shard
                    .current_segment
                    .store(self.segment_of(i, next_local), Ordering::Relaxed);
            }
        }
        let max_segment = self
            .shards
            .iter()
            .enumerate()
            .map(|(i, shard)| self.segment_of(i, shard.local_counter.load(Ordering::Relaxed)))
            .max()
            .unwrap_or(0);
        self.segment_allocator
            .store(max_segment + 1, Ordering::Relaxed);
        Ok(())
    }

    // ==================== Verification ====================

    pub fn verify_invariants(&self) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().verify_invariants()?;
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
        for num_shards in [1usize, 2, 4, 8, 16] {
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
        for num_shards in [1usize, 2, 4, 8] {
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
