use parking_lot::RwLock;

use super::core::{VertexTable, VertexTableConfig};

mod maintenance;
pub(crate) mod persistence;
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
        let ts_insert = 100;
        let ts_delete = 200;
        insert_with_name(&table, "gc_test", ts_insert);
        table.delete("gc_test", ts_delete).unwrap();
        let (gc_vertices, gc_versions) = table.gc_detailed(250).unwrap();

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
        let ts_insert = 100;
        let ts_delete = 200;
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
        let (live, allocated) = table.id_hole_stats(250);
        assert_eq!((live, allocated), (70, 100));
        // A cutoff before the deletes sees no holes.
        let (live, allocated) = table.id_hole_stats(150);
        assert_eq!((live, allocated), (100, 100));

        // Physical removal + compaction re-densifies local IDs and resets
        // the allocation counters (same path as compact_vertex_remap).
        let (removed, mapping, _) = table
            .compact_with_cutoff_collect_mapping(ts_delete)
            .unwrap();
        assert_eq!(removed.len(), 30);
        assert!(!mapping.is_empty());

        let (live, allocated) = table.id_hole_stats(250);
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

    #[test]
    fn test_table_manifest_rejects_shard_count_mismatch() {
        let dir = std::env::temp_dir().join(format!("sharded_manifest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        insert_with_name(&table, "v_manifest", ts);
        table
            .flush(&dir, crate::compression::CompressionType::Zstd { level: 0 })
            .unwrap();

        assert!(dir.join("table_manifest.json").exists());

        let same = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        same.load(&dir).unwrap();
        assert!(same.get_internal_id("v_manifest", ts).is_some());

        let other = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 8);
        let err = other.load(&dir).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("num_shards"),
            "mismatch error must name the shard count: {msg}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_table_manifest_missing_stays_loadable() {
        let dir =
            std::env::temp_dir().join(format!("sharded_manifest_legacy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        insert_with_name(&table, "v_legacy", TEST_TS);
        table
            .flush(&dir, crate::compression::CompressionType::Zstd { level: 0 })
            .unwrap();
        // True legacy layout: neither the layout manifest nor the commit
        // manifest exists. A present commit manifest pins its file set, so
        // removing only the layout manifest is an incomplete commit and
        // must refuse instead.
        std::fs::remove_file(dir.join("table_manifest.json")).unwrap();
        std::fs::remove_file(dir.join("commit_manifest.json")).unwrap();

        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        reloaded.load(&dir).unwrap();
        assert!(reloaded.get_internal_id("v_legacy", TEST_TS).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_low_fragmentation_keeps_row_ids_stable() {
        // Single shard, 5 rows, 1 delete: hole rate 0.2 stays below the
        // watermark, so compaction must not move any live row and the
        // edge cascade sees an empty mapping (zero edge writes).
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts_insert = 100;
        let ts_delete = 200;
        let mut before = std::collections::HashMap::new();
        for i in 0..5 {
            let name = format!("stable_{}", i);
            let id = insert_with_name(&table, &name, ts_insert);
            before.insert(name, id);
        }
        table.delete("stable_0", ts_delete).unwrap();
        let (removed, mapping, _) = table
            .compact_with_cutoff_collect_mapping(ts_delete)
            .unwrap();
        assert!(
            removed.is_empty() && mapping.is_empty(),
            "below-watermark compaction must skip the shard without remapping live rows"
        );
        for i in 1..5 {
            let name = format!("stable_{}", i);
            assert_eq!(
                table.get_internal_id(&name, ts_delete),
                before.get(&name).copied()
            );
        }
        assert_eq!(table.get_internal_id("stable_0", ts_delete), None);
    }

    #[test]
    fn test_stable_collect_moves_no_live_rows_above_watermark() {
        // 10 rows, 4 deletes: hole rate 0.4 exceeds the watermark, so the
        // legacy path would re-densify. The stable path absorbs holes
        // through the free stack and returns an empty mapping (zero edge
        // rewrites) with survivors pinned to their ids.
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts_insert = 100;
        let ts_delete = 200;
        let mut before = std::collections::HashMap::new();
        for i in 0..10 {
            let name = format!("row_{}", i);
            let id = insert_with_name(&table, &name, ts_insert);
            before.insert(name, id);
        }
        for i in 0..4 {
            table.delete(&format!("row_{}", i), ts_delete).unwrap();
        }
        let (removed, mapping, _) = table.compact_with_cutoff_stable_collect(ts_delete).unwrap();
        assert_eq!(removed.len(), 4);
        assert!(
            mapping.is_empty(),
            "stable collection must produce zero edge rewrites"
        );
        for i in 4..10 {
            let name = format!("row_{}", i);
            assert_eq!(
                table.get_internal_id(&name, ts_delete),
                before.get(&name).copied(),
                "survivors never move under stable row ids"
            );
        }
    }

    #[test]
    fn test_stable_holes_are_reused_by_new_inserts() {
        // Identifier monotonicity plus hole reuse: new inserts fill free
        // slots without shifting survivors.
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts_insert = 100;
        let ts_delete = 200;
        let ts_reinsert = 300;
        let mut survivor_ids = std::collections::HashMap::new();
        for i in 0..5 {
            let name = format!("hole_{}", i);
            insert_with_name(&table, &name, ts_insert);
        }
        table.delete("hole_1", ts_delete).unwrap();
        table.delete("hole_3", ts_delete).unwrap();
        let (removed, mapping, _) = table.compact_with_cutoff_stable_collect(ts_delete).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(mapping.is_empty());
        for name in ["hole_0", "hole_2", "hole_4"] {
            survivor_ids.insert(
                name.to_string(),
                table.get_internal_id(name, ts_delete).unwrap(),
            );
        }
        insert_with_name(&table, "hole_new_a", ts_reinsert);
        insert_with_name(&table, "hole_new_b", ts_reinsert);
        for (name, id) in &survivor_ids {
            assert_eq!(
                table.get_internal_id(name, ts_reinsert),
                Some(*id),
                "reused holes must not shift survivors"
            );
        }
        assert_eq!(table.id_hole_stats(ts_reinsert).0, 5);
    }

    #[test]
    fn test_concurrent_same_key_insert_allocates_once() {
        use std::sync::Arc;
        let table = Arc::new(ShardedVertexTable::with_config(
            1,
            "person".to_string(),
            test_schema(),
            8,
        ));
        let ts = TEST_TS;
        let mut handles = Vec::new();
        for _ in 0..8 {
            let t = Arc::clone(&table);
            handles.push(std::thread::spawn(move || {
                t.insert(
                    "hot_key",
                    &[("name".to_string(), Value::from("hot_key"))],
                    ts,
                )
                .map(|_| ())
            }));
        }
        let mut oks = 0usize;
        for h in handles {
            if h.join().unwrap().is_ok() {
                oks += 1;
            }
        }
        assert_eq!(oks, 1, "same-key concurrent inserts allocate exactly once");
        assert_eq!(table.total_count(), 1);
    }

    #[test]
    fn test_pk_delta_baseline_plus_incremental_reload() {
        use crate::vertex::vertex_table::sharded::persistence::COMMIT_MANIFEST_FILE_NAME;
        let base = std::env::temp_dir().join(format!("pk_base_{}", std::process::id()));
        let incr = std::env::temp_dir().join(format!("pk_incr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&incr);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        for i in 0..4 {
            insert_with_name(&table, &format!("base_{}", i), ts);
        }
        table
            .flush(
                &base,
                crate::compression::CompressionType::Zstd { level: 0 },
            )
            .unwrap();

        for i in 0..3 {
            insert_with_name(&table, &format!("incr_{}", i), ts);
        }
        table.delete("base_0", ts).unwrap();
        table
            .flush_incremental_with_epoch(
                &incr,
                crate::compression::CompressionType::Zstd { level: 0 },
                2,
                Some(1),
            )
            .unwrap();
        // Incremental shards carry the pk delta instead of a rewritten full
        // index for shards whose ids did not move.
        let mut saw_delta = false;
        for entry in std::fs::read_dir(&incr).unwrap().flatten() {
            let shard = entry.path();
            if shard.is_dir()
                && (shard.join("id_indexer.delta").exists()
                    || shard.join("id_indexer.bin").exists())
            {
                saw_delta = true;
            }
        }
        assert!(saw_delta, "incremental must persist pk changes");

        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        reloaded.load(&base).unwrap();
        reloaded.apply_delta_pages(&incr).unwrap();
        for i in 0..4 {
            let visible = reloaded
                .get_internal_id(&format!("base_{}", i), ts)
                .is_some();
            assert_eq!(visible, i != 0, "baseline delete must survive the overlay");
        }
        for i in 0..3 {
            assert!(reloaded
                .get_internal_id(&format!("incr_{}", i), ts)
                .is_some());
        }

        // A corrupt manifest-listed delta refuses the open with epoch info...
        for entry in std::fs::read_dir(&incr).unwrap().flatten() {
            let delta = entry.path().join("id_indexer.delta");
            if delta.exists() {
                std::fs::write(&delta, b"corrupt").unwrap();
                let err = reloaded.apply_delta_pages(&incr).unwrap_err().to_string();
                assert!(
                    err.contains('2'),
                    "strict delta error must carry the epoch: {err}"
                );
                break;
            }
        }
        // ...while the legacy path (no commit manifest) discards the delta
        // and keeps the baseline.
        let _ = std::fs::remove_file(incr.join(COMMIT_MANIFEST_FILE_NAME));
        let legacy = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        legacy.load(&base).unwrap();
        legacy.apply_delta_pages(&incr).unwrap();
        assert!(legacy.get_internal_id("base_1", ts).is_some());

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&incr);
    }

    #[test]
    fn test_insert_batch_str_groups_shards_and_aligns_results() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 8);
        let ts = TEST_TS;
        let names: Vec<String> = (0..100).map(|i| format!("b_{}", i)).collect();
        let props: Vec<Vec<(String, Value)>> = names
            .iter()
            .map(|n| vec![("name".to_string(), Value::from(n.as_str()))])
            .collect();
        let rows: Vec<(&str, &[(String, Value)])> = names
            .iter()
            .zip(props.iter())
            .map(|(n, p)| (n.as_str(), p.as_slice()))
            .collect();
        let results = table.insert_batch_str(&rows, ts);
        assert_eq!(results.len(), rows.len());
        let mut ids = std::collections::HashSet::new();
        for result in &results {
            assert!(ids.insert(result.as_ref().unwrap()), "duplicate global id");
        }
        for name in &names {
            assert!(table.get_internal_id(name, ts).is_some());
        }
        assert_eq!(table.total_count(), 100);
    }

    #[test]
    fn test_insert_batch_reports_per_row_errors() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        insert_with_name(&table, "dup", ts);
        let pa = vec![("name".to_string(), Value::from("dup"))];
        let pb = vec![("name".to_string(), Value::from("fresh"))];
        let rows: Vec<(&str, &[(String, Value)])> =
            vec![("fresh", pb.as_slice()), ("dup", pa.as_slice())];
        let results = table.insert_batch_str(&rows, ts);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(table.get_internal_id("fresh", ts).is_some());
    }
}
