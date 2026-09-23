use parking_lot::RwLock;

use super::core::{VertexTable, VertexTableConfig};

mod maintenance;
mod persistence;
mod read;
mod routing;
mod schema;
mod write;

pub struct ShardedVertexTable {
    shards: Vec<RwLock<VertexTable>>,
    num_shards: usize,
    label: graphdb_core::types::LabelId,
    label_name: String,
}

impl ShardedVertexTable {
    pub fn new(
        label: graphdb_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
    ) -> Self {
        Self::with_config(label, label_name, schema, routing::default_num_shards())
    }

    pub fn with_config(
        label: graphdb_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
        num_shards: usize,
    ) -> Self {
        let num_shards = num_shards.clamp(1, routing::MAX_SHARDS).next_power_of_two();
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

    #[cfg(test)]
    pub(crate) fn verify_invariants(&self) -> graphdb_core::StorageResult<()> {
        use graphdb_core::error::storage::StorageErrorKind;

        for shard in &self.shards {
            let table = shard.read();
            let id_count = table.id_indexer.len();

            for (key, idx) in table.id_indexer.iter() {
                let start_ts = table.timestamps.get_start_ts(idx);
                if start_ts.is_none() {
                    return Err(graphdb_core::StorageError::new(
                        StorageErrorKind::StorageError,
                        format!("ID {} for key {:?} missing in timestamps", idx, key),
                    ));
                }
            }

            for idx in 0..table.timestamps.size() {
                if let Some(_start_ts) = table.timestamps.get_start_ts(idx as u32) {
                    let key = table.id_indexer.get_key(idx as u32);
                    if key.is_none() {
                        return Err(graphdb_core::StorageError::new(
                            StorageErrorKind::StorageError,
                            format!("Timestamp entry {} missing in id_indexer", idx),
                        ));
                    }
                }
            }

            if table.columns.row_count() != id_count {
                return Err(graphdb_core::StorageError::new(
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
}

#[cfg(test)]
mod tests {
    use super::routing::{decode_id, encode_id, SEGMENT_SLOTS};
    use super::*;
    use graphdb_core::types::MAX_TIMESTAMP;
    const TEST_TS: Timestamp = MAX_TIMESTAMP - 1;
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::Timestamp;
    use graphdb_core::{DataType, Value};
    use std::sync::Arc;

    fn test_schema() -> crate::vertex::VertexSchema {
        crate::vertex::VertexSchema {
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
            .flush(&dir, crate::compression::CompressionType::Zstd { level: 0 })
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
                "Alice",
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

    #[test]
    fn test_table_level_snapshot_pin_counts() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 8);
        assert_eq!(table.active_snapshot_count(), 0);
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
        let (gc_vertices, gc_versions) = table.gc_detailed(150).unwrap();

        let count = gc_vertices + gc_versions;
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
        let (removed, mapping) = table
            .compact_with_cutoff_collect_mapping(ts_insert)
            .unwrap();
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
