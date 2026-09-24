use super::ShardedVertexTable;
use crate::vertex::IdKey;
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult, Value};

impl ShardedVertexTable {
    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_str(external_id);
        // Compare-and-swap fast path: a read-lock dedup hit returns the
        // duplicate error without an exclusive section. Misses (and
        // timestamp-deleted keys) fall through to the write-locked atomic
        // path, which rechecks before allocating, so concurrent same-key
        // inserts still allocate exactly once.
        let key = IdKey::Text(external_id.to_string());
        if self.shards[idx].read().is_duplicate(&key, ts) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }
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
        let key = IdKey::Int(external_id);
        if self.shards[idx].read().is_duplicate(&key, ts) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }
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

    /// Shard-grouped batch apply of a string-keyed insert batch.
    ///
    /// Rows are first routed to their owning shards without taking any lock
    /// (the routing step), then each non-empty shard group is applied under
    /// a single write-lock hold in input order (the merge step). Results are
    /// aligned with the input: every row is attempted even after earlier
    /// failures, so the caller rolls back every `Ok` entry when any `Err`
    /// is present. Duplicate keys fail per row exactly as [`insert`] does.
    ///
    /// [`insert`]: Self::insert
    pub fn insert_batch_str(
        &self,
        rows: &[(&str, &[(String, Value)])],
        ts: Timestamp,
    ) -> Vec<StorageResult<u32>> {
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.num_shards];
        for (pos, (external_id, _)) in rows.iter().enumerate() {
            by_shard[self.shard_index_by_str(external_id)].push(pos);
        }
        let mut out: Vec<Option<StorageResult<u32>>> = (0..rows.len()).map(|_| None).collect();
        for (shard_idx, positions) in by_shard.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let mut table = self.shards[shard_idx].write();
            for &pos in positions {
                let (external_id, properties) = &rows[pos];
                let result = table
                    .insert(external_id, properties, ts)
                    .map(|local_id| self.record_allocation(shard_idx, local_id));
                out[pos] = Some(result);
            }
        }
        out.into_iter()
            .map(|slot| slot.expect("every input row is routed once"))
            .collect()
    }

    /// Shard-grouped staged apply of an integer-keyed insert batch. Same
    /// staging/merge contract as [`insert_batch_str`].
    ///
    /// [`insert_batch_str`]: Self::insert_batch_str
    pub fn insert_batch_i64(
        &self,
        rows: &[(i64, &[(String, Value)])],
        ts: Timestamp,
    ) -> Vec<StorageResult<u32>> {
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.num_shards];
        for (pos, (external_id, _)) in rows.iter().enumerate() {
            by_shard[self.shard_index_by_i64(*external_id)].push(pos);
        }
        let mut out: Vec<Option<StorageResult<u32>>> = (0..rows.len()).map(|_| None).collect();
        for (shard_idx, positions) in by_shard.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let mut table = self.shards[shard_idx].write();
            for &pos in positions {
                let (external_id, properties) = &rows[pos];
                let result = table
                    .insert_by_i64(*external_id, properties, ts)
                    .map(|local_id| self.record_allocation(shard_idx, local_id));
                out[pos] = Some(result);
            }
        }
        out.into_iter()
            .map(|slot| slot.expect("every input row is routed once"))
            .collect()
    }
}
