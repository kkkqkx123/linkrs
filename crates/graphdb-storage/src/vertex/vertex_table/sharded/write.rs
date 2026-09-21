use super::ShardedVertexTable;
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageResult, Value};

impl ShardedVertexTable {
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
}
