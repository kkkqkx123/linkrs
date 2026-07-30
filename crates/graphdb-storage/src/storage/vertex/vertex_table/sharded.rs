use std::path::Path;

use parking_lot::{Mutex, RwLock};

use super::core::{VertexTable, VertexTableConfig};
use crate::core::types::Timestamp;
use crate::core::{StorageResult, Value};
use crate::storage::compression::CompressionType;
use crate::storage::mvcc::SnapshotHandle;
use crate::storage::schema::ChangeDetails;
use crate::storage::types::StoragePropertyDef;
use crate::storage::vertex::{IdKey, VertexRecord};

const DEFAULT_NUM_SHARDS: usize = 8;
const SHARD_BITS: u32 = 4;
const SHARD_MASK: u32 = (1 << SHARD_BITS) - 1;
const LOCAL_ID_MASK: u32 = !(SHARD_MASK << (32 - SHARD_BITS));
const MAX_SHARDS: usize = 1 << SHARD_BITS;

fn encode_id(shard: usize, local_id: u32) -> u32 {
    debug_assert!(shard < MAX_SHARDS);
    (shard as u32) << (32 - SHARD_BITS) | (local_id & LOCAL_ID_MASK)
}

fn decode_id(global_id: u32) -> (usize, u32) {
    let shard = (global_id >> (32 - SHARD_BITS)) as usize;
    let local_id = global_id & LOCAL_ID_MASK;
    (shard, local_id)
}

struct Shard {
    table: Mutex<VertexTable>,
}

pub struct ShardedVertexTable {
    shards: Vec<Shard>,
    num_shards: usize,
    label: crate::core::types::LabelId,
    label_name: String,
    schema: RwLock<crate::storage::vertex::VertexSchema>,
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
            });
        }
        Self {
            shards,
            num_shards,
            label,
            label_name,
            schema: RwLock::new(schema),
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

    // ==================== Write Operations ====================

    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_str(external_id);
        let mut table = self.shards[idx].table.lock();
        let local_id = table.insert(external_id, properties, ts)?;
        Ok(encode_id(idx, local_id))
    }

    pub fn insert_by_i64(
        &self,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let mut table = self.shards[idx].table.lock();
        let local_id = table.insert_by_i64(external_id, properties, ts)?;
        Ok(encode_id(idx, local_id))
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
        let (idx, local_id) = decode_id(global_id);
        let mut table = self.shards[idx].table.lock();
        table.update_property(local_id, col_name, value, ts)
    }

    // ==================== Read Operations ====================

    pub fn get_by_internal_id(&self, global_id: u32, ts: Timestamp) -> Option<VertexRecord> {
        let (idx, local_id) = decode_id(global_id);
        let table = self.shards[idx].table.lock();
        table.get_by_internal_id(local_id, ts)
    }

    pub fn get_internal_id(&self, external_id: &str, ts: Timestamp) -> Option<u32> {
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].table.lock();
        let local_id = table.get_internal_id(external_id, ts)?;
        Some(encode_id(idx, local_id))
    }

    pub fn get_internal_id_by_i64(&self, external_id: i64, ts: Timestamp) -> Option<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].table.lock();
        let local_id = table.get_internal_id_by_i64(external_id, ts)?;
        Some(encode_id(idx, local_id))
    }

    pub fn total_count(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.table.lock().total_count();
        }
        total
    }

    pub fn scan(&self, ts: Timestamp) -> Vec<VertexRecord> {
        let mut all = Vec::new();
        for (shard_idx, shard) in self.shards.iter().enumerate() {
            let table = shard.table.lock();
            for mut record in table.scan(ts) {
                record.internal_id = encode_id(shard_idx, record.internal_id);
                all.push(record);
            }
        }
        all
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
        self.schema.read().clone()
    }

    pub fn set_schema(&self, schema: crate::storage::vertex::VertexSchema) {
        for shard in &self.shards {
            shard.table.lock().set_schema(schema.clone());
        }
        *self.schema.write() = schema;
    }

    pub fn schema_mut(&self) -> crate::storage::vertex::VertexSchema {
        self.schema.read().clone()
    }

    pub fn apply_schema(&self, schema: crate::storage::vertex::VertexSchema) {
        for shard in &self.shards {
            shard.table.lock().set_schema(schema.clone());
        }
        *self.schema.write() = schema;
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
        let (idx, local_id) = decode_id(global_id);
        let table = self.shards[idx].table.lock();
        table.get_projected_by_internal_id(local_id, ts, projection)
    }

    pub fn get_internal_id_raw(&self, external_id: &str) -> Option<u32> {
        let idx = self.shard_index_by_str(external_id);
        let table = self.shards[idx].table.lock();
        let local_id = table.get_internal_id_raw(external_id)?;
        Some(encode_id(idx, local_id))
    }

    pub fn get_internal_id_by_i64_raw(&self, external_id: i64) -> Option<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let table = self.shards[idx].table.lock();
        let local_id = table.get_internal_id_by_i64_raw(external_id)?;
        Some(encode_id(idx, local_id))
    }

    pub fn get_external_id(&self, global_id: u32, ts: Timestamp) -> Option<IdKey> {
        let (idx, local_id) = decode_id(global_id);
        let table = self.shards[idx].table.lock();
        table.get_external_id(local_id, ts)
    }

    pub fn get_external_id_raw(&self, global_id: u32) -> Option<IdKey> {
        let (idx, local_id) = decode_id(global_id);
        let table = self.shards[idx].table.lock();
        table.get_external_id_raw(local_id)
    }

    // ==================== Additional Write Operations ====================

    pub fn delete_by_internal_id(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = decode_id(global_id);
        let mut table = self.shards[idx].table.lock();
        table.delete_by_internal_id(local_id, ts)
    }

    pub fn revert_delete(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = decode_id(global_id);
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
        let (idx, local_id) = decode_id(global_id);
        let mut table = self.shards[idx].table.lock();
        table.update_property_by_id(local_id, col_id, value, ts)
    }

    pub fn batch_delete(&self, external_ids: &[&str], ts: Timestamp) -> StorageResult<usize> {
        let mut total = 0;
        for id in external_ids {
            if self.delete(id, ts).is_ok() {
                total += 1;
            }
        }
        Ok(total)
    }

    pub fn batch_delete_i64(&self, external_ids: &[i64], ts: Timestamp) -> StorageResult<usize> {
        let mut total = 0;
        for id in external_ids {
            if self.delete_by_i64(*id, ts).is_ok() {
                total += 1;
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
        {
            let mut schema = self.schema.write();
            if !schema.properties.iter().any(|p| p.name == prop.name) {
                schema.properties.push(prop);
            }
        }
        Ok(())
    }

    pub fn remove_property(&self, prop_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().remove_property(prop_name)?;
        }
        {
            let mut schema = self.schema.write();
            schema.properties.retain(|p| p.name != prop_name);
        }
        Ok(())
    }

    pub fn rename_property(&self, old_name: &str, new_name: &str) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().rename_property(old_name, new_name)?;
        }
        {
            let mut schema = self.schema.write();
            if let Some(prop) = schema.properties.iter_mut().find(|p| p.name == old_name) {
                prop.name = new_name.to_string();
            }
        }
        Ok(())
    }

    pub fn rebuild_schema_change_from_redo(&self, details: ChangeDetails) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().rebuild_schema_change_from_redo(details.clone())?;
        }
        Ok(())
    }

    // ==================== Compaction ====================

    pub fn compact_coordinated(&self) -> StorageResult<()> {
        for shard in &self.shards {
            shard.table.lock().compact_coordinated()?;
        }
        Ok(())
    }

    pub fn compact_timestamps(&self) -> std::collections::HashMap<u32, u32> {
        let mut combined = std::collections::HashMap::new();
        for shard in &self.shards {
            let map = shard.table.lock().compact_timestamps();
            combined.extend(map);
        }
        combined
    }

    pub fn compact_with_ts_collect(&self, ts: Timestamp) -> StorageResult<Vec<IdKey>> {
        let mut all = Vec::new();
        for shard in &self.shards {
            all.extend(shard.table.lock().compact_with_ts_collect(ts)?);
        }
        Ok(all)
    }

    // ==================== Version History ====================

    pub fn version_history_ref(&self) -> std::sync::Arc<std::sync::Mutex<crate::storage::schema::LabelVersionHistory>> {
        self.shards[0].table.lock().version_history_ref()
    }

    // ==================== Persistence ====================

    pub fn flush<P: AsRef<Path>>(&self, path: P, compression: CompressionType) -> StorageResult<()> {
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
                shard.table.lock().load(&shard_dir)?;
            }
        }
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
    use std::sync::Arc;
    use crate::core::{DataType, Value};
    use crate::storage::types::StoragePropertyDef;

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
        for shard in 0..16 {
            for local in [0, 1, 42, 0x0FFFFFFF] {
                let e = encode_id(shard, local);
                let (s, l) = decode_id(e);
                assert_eq!(s, shard);
                assert_eq!(l, local);
            }
        }
    }

    #[test]
    fn test_insert_and_read() {
        let table = ShardedVertexTable::with_config(1, "person".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        let id = table
            .insert("alice", &[("name".to_string(), Value::from("Alice")), ("age".to_string(), Value::from(30i64))], ts)
            .unwrap();
        let record = table.get_by_internal_id(id, ts).unwrap();
        assert_eq!(record.properties.len(), 2);
    }

    fn insert_with_name(table: &ShardedVertexTable, name: &str, ts: Timestamp) -> u32 {
        table.insert(name, &[("name".to_string(), Value::from(name))], ts).unwrap()
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
            1, "person".to_string(), test_schema(), 8,
        ));
        let ts = TEST_TS;
        let t1 = Arc::clone(&table);
        let t2 = Arc::clone(&table);
        let h1 = std::thread::spawn(move || {
            for i in 0..100 {
                t1.insert(&format!("user_{}", i), &[("name".to_string(), Value::from(format!("user_{}", i)))], ts).unwrap();
            }
        });
        let h2 = std::thread::spawn(move || {
            for i in 100..200 {
                t2.insert(&format!("user_{}", i), &[("name".to_string(), Value::from(format!("user_{}", i)))], ts).unwrap();
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
        let table = ShardedVertexTable::with_config(
            1, "t".to_string(), test_schema(), 16,
        );
        let ts = TEST_TS;
        let mut ids = std::collections::HashSet::new();
        for i in 0..200 {
            let id = insert_with_name(&table, &format!("unique_{}", i), ts);
            assert!(ids.insert(id), "duplicate internal_id: {}", id);
        }
        assert_eq!(ids.len(), 200);
    }
}
