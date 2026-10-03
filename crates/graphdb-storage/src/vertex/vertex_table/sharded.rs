use parking_lot::RwLock;

use super::core::{VertexTable, VertexTableConfig};
use super::sharded::routing::ShardLayout;

pub(crate) mod maintenance;
pub(crate) mod persistence;
mod read;
pub(crate) mod routing;
mod schema;
pub(crate) mod write;

pub(crate) use write::CommitApplied;

/// Read-only redistribution preview: live rows plus their distribution under
/// the target shard count. Produced without copying any row; the rebuild
/// entry reuses it as its pre-copy gate and checks the produced mapping
/// against it afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReshardDryRun {
    pub live_rows: usize,
    pub new_num_shards: usize,
    pub per_shard_rows: Vec<usize>,
}

pub struct ShardedVertexTable {
    shards: Vec<RwLock<VertexTable>>,
    layout: ShardLayout,
    label: graphdb_core::types::LabelId,
    label_name: String,
    /// Redistribution generation of this table lineage. Fresh tables are
    /// generation zero; each offline redistribution bumps it. Persisted in
    /// the table manifest and pinned in the commit manifest so the open
    /// path refuses checkpoints mixed in from another generation.
    generation: u64,
    /// Wall-clock milliseconds of the last full baseline flush. Feeds the
    /// flush trigger's baseline-age branch; zero means never flushed.
    last_full_flush_ms: std::sync::atomic::AtomicU64,
}

impl ShardedVertexTable {
    pub fn new(
        label: graphdb_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
    ) -> Self {
        Self::with_estimate(label, label_name, schema, 1, None)
    }

    /// Build a new table from an estimated row count plus a parallelism cap.
    /// Small estimates stay on one shard; large estimates keep the
    /// parallelism-shaped layout. Opened tables ignore this and adopt the
    /// layout pinned in their manifest.
    pub fn with_estimate(
        label: graphdb_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
        parallelism_shards: usize,
        estimated_rows: Option<u64>,
    ) -> Self {
        Self::with_layout(
            label,
            label_name,
            schema,
            ShardLayout::for_new_table_with_estimate(parallelism_shards, estimated_rows),
            0,
        )
    }

    /// Build a table with an explicit shard count. Test and offline
    /// redistribution only; production creation goes through `with_estimate`.
    #[allow(dead_code)]
    pub fn with_config(
        label: graphdb_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
        num_shards: usize,
    ) -> Self {
        Self::with_layout(
            label,
            label_name,
            schema,
            ShardLayout::for_new_table(num_shards),
            0,
        )
    }

    /// Build a table under an explicit versioned layout. New tables use
    /// [`ShardLayout::for_new_table_with_estimate`] with generation zero;
    /// opened tables use the layout and generation pinned in their manifest.
    pub(crate) fn with_layout(
        label: graphdb_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
        layout: ShardLayout,
        generation: u64,
    ) -> Self {
        let mut shards = Vec::with_capacity(layout.num_shards);
        for _ in 0..layout.num_shards {
            shards.push(RwLock::new(VertexTable::with_config(
                label,
                label_name.clone(),
                schema.clone(),
                VertexTableConfig::default(),
            )));
        }
        Self {
            shards,
            layout,
            label,
            label_name,
            generation,
            last_full_flush_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn verify_invariants(&self) -> graphdb_core::StorageResult<()> {
        use graphdb_core::error::storage::StorageErrorKind;

        for shard in &self.shards {
            let table = shard.read();
            let id_count = table.id_indexer.len();

            for (key, idx) in table.id_indexer.iter() {
                let start_ts = table.timestamps.read().get_start_ts(idx);
                if start_ts.is_none() {
                    return Err(graphdb_core::StorageError::new(
                        StorageErrorKind::StorageError,
                        format!("ID {} for key {:?} missing in timestamps", idx, key),
                    ));
                }
            }

            for idx in 0..table.timestamps.read().size() {
                if let Some(_start_ts) = table.timestamps.read().get_start_ts(idx as u32) {
                    let key = table.id_indexer.get_key(idx as u32);
                    if key.is_none() {
                        return Err(graphdb_core::StorageError::new(
                            StorageErrorKind::StorageError,
                            format!("Timestamp entry {} missing in id_indexer", idx),
                        ));
                    }
                }
            }

            // Stable row ids leave holes: absorbed deletes keep their
            // column rows and timestamp capacity until free-stack reuse,
            // so the column store may be wider than the live key count.
            if table.columns.row_count() < id_count {
                return Err(graphdb_core::StorageError::new(
                    StorageErrorKind::StorageError,
                    format!(
                        "Column count ({}) below id_indexer.len() ({})",
                        table.columns.row_count(),
                        id_count
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Offline redistribution to a new shard count.
    ///
    /// Reads every live row at the maximum timestamp and rebuilds it in a
    /// fresh table with `new_num_shards`, returning the rebuilt table plus
    /// the old-global to new-global internal id mapping the rebuild
    /// produced. Row creation stamps travel with their rows so snapshot
    /// reads keep their original visibility lower bound; per-value version
    /// chains stay memory-only and are not migrated. The source stays
    /// untouched; the caller flushes the returned table and checkpoints it
    /// as the new baseline, then retires the old checkpoint directory.
    /// Online shard count changes stay rejected by the table manifest; this
    /// is the only adjustment outlet.
    ///
    /// Offline fence: the caller must hold the maintenance barrier with no
    /// concurrent writes, and no staged schema change may be pending on any
    /// shard. A pending schema change rejects the rebuild so a half-applied
    /// schema cannot leak into the new shard layout.
    ///
    /// The rebuilt table carries the next redistribution generation, so a
    /// checkpoint flushed from it can never be mistaken for one from the
    /// source lineage at open.
    ///
    /// The read-only precheck runs first: illegal layouts, pending schema
    /// changes, malformed keys, index entries without a retrievable record,
    /// and target capacity overflows fail before any row is copied. After
    /// the copy the produced mapping must cover every live row, otherwise
    /// the rebuild fails instead of handing out a partial edge-translation
    /// map.
    pub fn dry_run_reshard_to(
        &self,
        new_num_shards: usize,
    ) -> graphdb_core::StorageResult<ReshardDryRun> {
        use graphdb_core::types::MAX_TIMESTAMP;
        let target = ShardLayout::for_new_table(new_num_shards);
        if !target.is_consistent() {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "reshard refused: target layout for {} shards is inconsistent",
                new_num_shards
            )));
        }
        if target == self.layout {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "reshard is a no-op: table already uses {} shards",
                self.layout.num_shards
            )));
        }
        for (idx, shard) in self.shards.iter().enumerate() {
            if shard.read().has_pending_schema_change() {
                return Err(graphdb_core::StorageError::invalid_operation(format!(
                    "reshard refused: shard {} holds a pending schema change; \
                     finish or abort it before offline redistribution",
                    idx
                )));
            }
        }
        let ts = MAX_TIMESTAMP - 1;
        let mut per_shard_rows = vec![0usize; target.num_shards];
        let mut live_rows = 0usize;
        let mask = target.num_shards - 1;
        for key in self.external_id_keys() {
            match &key {
                crate::vertex::IdKey::Text(name) => {
                    if name.len() > graphdb_core::types::VERTEX_ID_MAX_SIZE {
                        return Err(graphdb_core::StorageError::invalid_input(format!(
                            "reshard refused: text key of {} bytes exceeds the limit",
                            name.len()
                        )));
                    }
                    let Some(old_global) = self.get_internal_id(name, ts) else {
                        continue;
                    };
                    let Some(_) = self.get_by_internal_id_offline(old_global, ts) else {
                        return Err(graphdb_core::StorageError::invalid_operation(
                            "reshard refused: indexed text key maps to a missing record; \
                             refusing a partial edge-translation map"
                                .to_string(),
                        ));
                    };
                    let shard = (routing::fxhash(name) as usize) & mask;
                    per_shard_rows[shard] += 1;
                    live_rows += 1;
                }
                crate::vertex::IdKey::Int(n) => {
                    if *n < 0 {
                        return Err(graphdb_core::StorageError::invalid_input(format!(
                            "reshard refused: negative integer key {}",
                            n
                        )));
                    }
                    let Some(old_global) = self.get_internal_id_by_i64(*n, ts) else {
                        continue;
                    };
                    let Some(_) = self.get_by_internal_id_offline(old_global, ts) else {
                        return Err(graphdb_core::StorageError::invalid_operation(format!(
                            "reshard refused: indexed integer key {} maps to a missing record; \
                             refusing a partial edge-translation map",
                            n
                        )));
                    };
                    let shard = (routing::fxhash_i64(*n) as usize) & mask;
                    per_shard_rows[shard] += 1;
                    live_rows += 1;
                }
            }
        }
        let per_shard_capacity = (target.total_segments / target.num_shards as u32) as u64
            * target.segment_slots() as u64;
        for (shard, rows) in per_shard_rows.iter().enumerate() {
            if *rows as u64 > per_shard_capacity {
                return Err(graphdb_core::StorageError::invalid_operation(format!(
                    "reshard refused: target shard {} would hold {} rows beyond the address space {}",
                    shard, rows, per_shard_capacity
                )));
            }
        }
        Ok(ReshardDryRun {
            live_rows,
            new_num_shards: target.num_shards,
            per_shard_rows,
        })
    }

    pub fn reshard_to(
        &self,
        new_num_shards: usize,
    ) -> graphdb_core::StorageResult<(Self, std::collections::HashMap<u32, u32>)> {
        use graphdb_core::types::MAX_TIMESTAMP;
        let preview = self.dry_run_reshard_to(new_num_shards)?;
        let target = ShardLayout::for_new_table(preview.new_num_shards);
        let schema = self.schema();
        let rebuilt = Self::with_layout(
            self.label,
            self.label_name.clone(),
            schema,
            target,
            self.generation.saturating_add(1),
        );
        let ts = MAX_TIMESTAMP - 1;
        let mut id_mapping: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for key in self.external_id_keys() {
            let (old_global, record, orig_create) = match &key {
                crate::vertex::IdKey::Text(name) => {
                    let Some(old_global) = self.get_internal_id(name, ts) else {
                        continue;
                    };
                    let Some(record) = self.get_by_internal_id_offline(old_global, ts) else {
                        continue;
                    };
                    let create = self
                        .row_timestamps(old_global)
                        .map(|(create, _)| create)
                        .unwrap_or(ts);
                    (old_global, record, create)
                }
                crate::vertex::IdKey::Int(n) => {
                    let Some(old_global) = self.get_internal_id_by_i64(*n, ts) else {
                        continue;
                    };
                    let Some(record) = self.get_by_internal_id_offline(old_global, ts) else {
                        continue;
                    };
                    let create = self
                        .row_timestamps(old_global)
                        .map(|(create, _)| create)
                        .unwrap_or(ts);
                    (old_global, record, create)
                }
            };
            let new_global = match &key {
                crate::vertex::IdKey::Text(name) => rebuilt.insert(name, &record.properties, ts)?,
                crate::vertex::IdKey::Int(n) => {
                    rebuilt.insert_by_i64(*n, &record.properties, ts)?
                }
            };
            if orig_create < ts {
                let (new_shard, new_local) = rebuilt.decode_id(new_global);
                rebuilt.shards[new_shard]
                    .read()
                    .backdate_row_for_reshard(new_local, orig_create);
            }
            id_mapping.insert(old_global, new_global);
        }
        if id_mapping.len() != preview.live_rows
            || rebuilt.approximate_total_count() != preview.live_rows
        {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "reshard incomplete: preview counted {} live rows but the rebuild mapped {} into {} rows; \
                 refusing a partial edge-translation map",
                preview.live_rows,
                id_mapping.len(),
                rebuilt.approximate_total_count(),
            )));
        }
        Ok((rebuilt, id_mapping))
    }
}

#[cfg(test)]
mod tests {
    use super::routing::{decode_id, encode_id, ShardLayout};
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
            let layout = ShardLayout::for_new_table(num_shards);
            let slots = layout.segment_slots();
            for shard in 0..num_shards {
                for local in [
                    0,
                    1,
                    42,
                    slots - 1,
                    slots,
                    slots * num_shards as u32,
                    u32::MAX / num_shards as u32,
                ] {
                    let e = encode_id(shard, local, layout);
                    let (s, l) = decode_id(e, layout);
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
        let n = table.layout().segment_slots() as usize + 32;
        for i in 0..n {
            let id = insert_with_name(&table, &format!("s_{}", i), ts);
            ids.push(id);
        }
        for (i, &id) in ids.iter().enumerate() {
            let record = table.get_by_internal_id_offline(id, ts).unwrap();
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
        assert_eq!(table.approximate_total_count(), n);
    }

    #[test]
    fn test_load_resumes_allocation() {
        let dir = std::env::temp_dir().join(format!("sharded_load_{}", std::process::id()));
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        let slots = table.layout().segment_slots();
        for i in 0..slots + 100 {
            insert_with_name(&table, &format!("v_{}", i), ts);
        }
        table
            .flush(&dir, crate::compression::CompressionType::Zstd { level: 0 })
            .unwrap();

        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        reloaded.load(&dir).unwrap();

        for i in 0..slots + 100 {
            assert!(reloaded.get_internal_id(&format!("v_{}", i), ts).is_some());
        }

        let new_id = insert_with_name(&reloaded, "v_new_after_load", ts);
        let new_id2 = insert_with_name(&reloaded, "v_new_after_load2", ts);
        assert_ne!(new_id, new_id2);
        assert!(reloaded.get_by_internal_id_offline(new_id, ts).is_some());
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
        let record = table.get_by_internal_id_offline(id, ts).unwrap();
        assert_eq!(record.properties.len(), 2);
    }

    fn insert_with_name(table: &ShardedVertexTable, name: &str, ts: Timestamp) -> u32 {
        table
            .insert(name, &[("name".to_string(), Value::from(name))], ts)
            .unwrap()
    }

    /// Guard over a version manager with no write in flight: every stamp is a
    /// slot the manager never owned, so the plain timestamp predicate applies.
    fn scan_guard<'a>(
        ts: Timestamp,
        manager: &'a graphdb_transaction::VersionManager,
    ) -> crate::mvcc_visibility::VisibilityGuard<'a> {
        crate::mvcc_visibility::VisibilityGuard::new(
            ts,
            crate::mvcc_visibility::PendingGate::new(manager, None),
        )
    }

    #[test]
    fn test_delete() {
        let table = ShardedVertexTable::with_config(1, "person".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        let id = insert_with_name(&table, "bob", ts);
        assert!(table.get_by_internal_id_offline(id, ts).is_some());
        let gid = table.get_internal_id("bob", ts).unwrap();
        table.delete_by_internal_id(gid, ts).unwrap();
        assert!(table.get_by_internal_id_offline(id, ts).is_none());
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
        assert_eq!(table.approximate_total_count(), 200);
    }

    #[test]
    fn test_scan() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        for i in 0..50 {
            insert_with_name(&table, &format!("k{}", i), ts);
        }
        let manager = graphdb_transaction::VersionManager::new();
        let results = table.scan(&scan_guard(ts, &manager));
        assert_eq!(results.len(), 50);
    }

    #[test]
    fn test_sharded_eviction_pressure_scan_matches_resident() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts = TEST_TS;
        for i in 0..200 {
            insert_with_name(&table, &format!("p_{}", i), ts);
        }
        let manager = graphdb_transaction::VersionManager::new();
        let expected = table.scan(&scan_guard(ts, &manager));
        assert_eq!(expected.len(), 200);
        // Fresh overlay chunks stay resident by design; the sharded pass
        // must still keep quota totals consistent with the full pass and
        // leave scans intact.
        let (full_evicted, full_freed) = table.evict_cold_chunks(u64::MAX);
        let (seg_evicted, seg_freed, _) = table.evict_cold_chunks_with_quota(u64::MAX, 1);
        assert_eq!((seg_evicted, seg_freed), (full_evicted, full_freed));
        let after = table.scan(&scan_guard(ts, &manager));
        assert_eq!(after.len(), expected.len());
        let mut before_ids: Vec<u32> = expected.iter().map(|r| r.internal_id).collect();
        let mut after_ids: Vec<u32> = after.iter().map(|r| r.internal_id).collect();
        before_ids.sort_unstable();
        after_ids.sort_unstable();
        assert_eq!(before_ids, after_ids);
    }

    #[test]
    fn test_sharded_eviction_concurrent_write_keeps_new_rows() {
        use std::sync::Arc;
        let table = Arc::new(ShardedVertexTable::with_config(
            1,
            "t".to_string(),
            test_schema(),
            4,
        ));
        let ts = TEST_TS;
        for i in 0..100 {
            insert_with_name(&table, &format!("c_{}", i), ts);
        }
        let writer = Arc::clone(&table);
        let handle = std::thread::spawn(move || {
            for i in 100..150 {
                insert_with_name(&writer, &format!("c_{}", i), ts);
            }
        });
        let _ = table.evict_cold_chunks(u64::MAX);
        handle.join().expect("writer thread failed");
        assert_eq!(table.approximate_total_count(), 150);
        let manager = graphdb_transaction::VersionManager::new();
        let results = table.scan(&scan_guard(ts, &manager));
        assert_eq!(results.len(), 150);
    }

    #[test]
    fn test_gc() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        let ts_insert = 100;
        let ts_delete = 200;
        insert_with_name(&table, "gc_test", ts_insert);
        let gid = table.get_internal_id("gc_test", ts_delete).unwrap();
        table.delete_by_internal_id(gid, ts_delete).unwrap();
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
        // Single shard: shard selection is hash-driven, so a multi-shard
        // table may skip low-fragmentation shards and remove only a subset.
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts_insert = 100;
        let ts_delete = 200;
        for i in 0..100 {
            insert_with_name(&table, &format!("v_{}", i), ts_insert);
        }
        let (live, allocated) = table.approximate_id_hole_stats(150);
        assert_eq!((live, allocated), (100, 100));

        for i in 0..30 {
            let gid = table
                .get_internal_id(&format!("v_{}", i), ts_delete)
                .unwrap();
            table.delete_by_internal_id(gid, ts_delete).unwrap();
        }
        // Deleted vertices leave holes: allocated stays at the high-water
        // mark, live only counts vertices not deleted at the cutoff.
        let (live, allocated) = table.approximate_id_hole_stats(250);
        assert_eq!((live, allocated), (70, 100));
        // A cutoff before the deletes sees no holes.
        let (live, allocated) = table.approximate_id_hole_stats(150);
        assert_eq!((live, allocated), (100, 100));

        // Physical removal + compaction re-densifies local IDs and resets
        // the allocation counters (same path as compact_vertex_remap).
        let (removed, mapping, _) = table
            .compact_with_cutoff_collect_mapping(ts_delete)
            .unwrap();
        assert_eq!(removed.len(), 30);
        assert!(!mapping.is_empty());

        let (live, allocated) = table.approximate_id_hole_stats(250);
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
    fn test_table_manifest_missing_refuses_open() {
        let dir =
            std::env::temp_dir().join(format!("sharded_manifest_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        insert_with_name(&table, "v_missing", TEST_TS);
        table
            .flush(&dir, crate::compression::CompressionType::Zstd { level: 0 })
            .unwrap();
        // Missing manifests refuse: global IDs embed the shard layout and
        // the commit manifest pins the file set.
        std::fs::remove_file(dir.join("table_manifest.json")).unwrap();
        std::fs::remove_file(dir.join("commit_manifest.json")).unwrap();

        let reloaded = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let err = reloaded.load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("manifest"),
            "missing manifest must refuse: {err}"
        );

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
        let gid = table.get_internal_id("stable_0", ts_delete).unwrap();
        table.delete_by_internal_id(gid, ts_delete).unwrap();
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
        // offline remap path would re-densify. The stable path absorbs holes
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
            let gid = table
                .get_internal_id(&format!("row_{}", i), ts_delete)
                .unwrap();
            table.delete_by_internal_id(gid, ts_delete).unwrap();
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
        let gid = table.get_internal_id("hole_1", ts_delete).unwrap();
        table.delete_by_internal_id(gid, ts_delete).unwrap();
        let gid = table.get_internal_id("hole_3", ts_delete).unwrap();
        table.delete_by_internal_id(gid, ts_delete).unwrap();
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
        assert_eq!(table.approximate_id_hole_stats(ts_reinsert).0, 5);
    }

    #[test]
    fn test_offline_and_stable_agree_on_removed_set_above_watermark() {
        let build = || {
            let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
            for i in 0..10 {
                insert_with_name(&table, &format!("row_{}", i), 100);
            }
            for i in 0..4 {
                let gid = table.get_internal_id(&format!("row_{}", i), 200).unwrap();
                table.delete_by_internal_id(gid, 200).unwrap();
            }
            table
        };
        let offline = build();
        let (offline_removed, offline_mapping, _) =
            offline.compact_with_cutoff_collect_mapping(200).unwrap();
        let stable = build();
        let (stable_removed, stable_mapping, _) =
            stable.compact_with_cutoff_stable_collect(200).unwrap();
        let mut offline_sorted = offline_removed
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>();
        offline_sorted.sort();
        let mut stable_sorted = stable_removed
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>();
        stable_sorted.sort();
        assert_eq!(offline_sorted, stable_sorted);
        assert!(!offline_mapping.is_empty());
        assert!(stable_mapping.is_empty());
        for i in 4..10 {
            let name = format!("row_{}", i);
            let offline_id = offline
                .get_internal_id(&name, 200)
                .expect("offline survivor");
            let stable_before = build().get_internal_id(&name, 200).expect("pre compact id");
            let stable_id = stable.get_internal_id(&name, 200).expect("stable survivor");
            assert_eq!(stable_id, stable_before);
            assert!(stable.get_by_internal_id_offline(stable_id, 200).is_some());
            assert!(offline
                .get_by_internal_id_offline(offline_id, 200)
                .is_some());
        }
    }

    #[test]
    fn test_gc_detailed_keeps_row_ids_stable() {
        // Background GC must absorb deletes without moving survivors: no
        // edge cascade is involved on this path by design.
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 1);
        let ts_insert = 100;
        let ts_delete = 200;
        let mut before = std::collections::HashMap::new();
        for i in 0..5 {
            let name = format!("gc_{}", i);
            let id = insert_with_name(&table, &name, ts_insert);
            before.insert(name, id);
        }
        let gid = table.get_internal_id("gc_1", ts_delete).unwrap();
        table.delete_by_internal_id(gid, ts_delete).unwrap();
        let gid = table.get_internal_id("gc_3", ts_delete).unwrap();
        table.delete_by_internal_id(gid, ts_delete).unwrap();
        let (reclaimed, _) = table.gc_detailed(ts_delete).unwrap();
        assert_eq!(reclaimed, 2);
        for i in [0, 2, 4] {
            let name = format!("gc_{}", i);
            assert_eq!(
                table.get_internal_id(&name, ts_delete),
                before.get(&name).copied(),
                "background GC must not move survivors"
            );
        }
        assert_eq!(table.get_internal_id("gc_1", ts_delete), None);
        table.verify_invariants().unwrap();
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
        assert_eq!(table.approximate_total_count(), 1);
    }

    #[test]
    fn test_pk_delta_baseline_plus_incremental_reload() {
        use crate::vertex::vertex_table::sharded::persistence::commit_manifest::COMMIT_MANIFEST_FILE_NAME;
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
        let gid = table.get_internal_id("base_0", ts).unwrap();
        table.delete_by_internal_id(gid, ts).unwrap();
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
        // ...and the repair entry enforces the same strict semantics.
        let _ = std::fs::remove_file(incr.join(COMMIT_MANIFEST_FILE_NAME));
        let reloaded_missing =
            ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        reloaded_missing.load(&base).unwrap();
        assert!(reloaded_missing.apply_delta_pages(&incr).is_err());

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
        assert_eq!(table.approximate_total_count(), 100);
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

    #[test]
    fn test_reshard_rebuilds_all_rows_under_new_count() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts = TEST_TS;
        for i in 0..20 {
            insert_with_name(&table, &format!("r_{}", i), ts);
        }
        let (rebuilt, mapping) = table.reshard_to(8).expect("reshard succeeds");
        assert_eq!(rebuilt.num_shards(), 8);
        assert_eq!(table.generation(), 0);
        assert_eq!(
            rebuilt.generation(),
            1,
            "redistribution must bump the lineage generation"
        );
        assert_eq!(rebuilt.approximate_total_count(), 20);
        assert_eq!(mapping.len(), 20);
        for i in 0..20 {
            let name = format!("r_{}", i);
            let old_id = table.get_internal_id(&name, ts).expect("old row");
            let new_id = rebuilt.get_internal_id(&name, ts).expect("rebuilt row");
            assert_eq!(mapping.get(&old_id), Some(&new_id));
            let old_record = table
                .get_by_internal_id_offline(old_id, ts)
                .expect("old record");
            let new_record = rebuilt
                .get_by_internal_id_offline(new_id, ts)
                .expect("new record");
            assert_eq!(old_record.properties, new_record.properties);
        }
        assert!(table.reshard_to(2).is_err());
    }

    #[test]
    fn test_storage_snapshot_combines_holes_and_buffers() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        for i in 0..6 {
            insert_with_name(&table, &format!("snap_{}", i), 100);
        }
        let gid = table.get_internal_id("snap_0", 200).unwrap();
        table.delete_by_internal_id(gid, 200).unwrap();
        let snapshot = table.storage_snapshot(250);
        assert_eq!(snapshot.live, 5);
        assert_eq!(snapshot.allocated, 6);
        assert_eq!(snapshot.holes, 1);
        assert_eq!(snapshot.pk_free_depth, 0);
    }

    #[test]
    fn test_table_cardinality_matches_hole_stats() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 4);
        for i in 0..10 {
            insert_with_name(&table, &format!("card_{}", i), 100);
        }
        for i in 0..3 {
            let gid = table.get_internal_id(&format!("card_{}", i), 200).unwrap();
            table.delete_by_internal_id(gid, 200).unwrap();
        }
        let snapshot = table.table_cardinality_at(250);
        assert_eq!((snapshot.live_rows, snapshot.allocated_slots), (7, 10));
        assert_eq!(snapshot.shard_count, 4);
        assert_eq!(snapshot.hole_count(), 3);
        assert!((snapshot.hole_rate() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn test_reshard_refuses_pending_schema_change() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        insert_with_name(&table, "r_0", TEST_TS);
        table
            .prepare_add_property_staged(StoragePropertyDef {
                name: "nick".to_string(),
                data_type: DataType::String,
                nullable: true,
                default_value: None,
            })
            .unwrap();
        let err = match table.reshard_to(4) {
            Ok(_) => panic!("reshard must refuse with pending schema change"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("pending schema change"),
            "fence must name the pending schema change: {err}"
        );
        table.abort_pending_schema_change();
        let (rebuilt, _) = table.reshard_to(4).expect("reshard succeeds after abort");
        assert_eq!(rebuilt.num_shards(), 4);
        assert!(rebuilt.get_internal_id("r_0", TEST_TS).is_some());
    }

    #[test]
    fn test_reshard_preserves_creation_stamps() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        for i in 0..10 {
            insert_with_name(&table, &format!("aged_{}", i), 100 + i);
        }
        let (rebuilt, _) = table.reshard_to(8).expect("reshard succeeds");
        for i in 0..10 {
            let name = format!("aged_{}", i);
            let new_id = rebuilt
                .get_internal_id(&name, TEST_TS)
                .expect("rebuilt row");
            let (create, _) = rebuilt.row_timestamps(new_id).expect("stamps");
            assert_eq!(create, 100 + i as u64);
            if 100 + i <= 105 {
                let record = rebuilt
                    .get_by_internal_id_offline(new_id, 105)
                    .expect("readable below the rebuild stamp");
                assert!(record.properties.iter().any(|(k, _)| k == "name"));
            } else {
                assert!(rebuilt.get_by_internal_id_offline(new_id, 105).is_none());
            }
        }
    }

    #[test]
    fn test_remap_only_rebuilds_over_watermark_shards() {
        use super::routing::fxhash;
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts_insert = 100;
        let ts_delete = 200;
        let mut shard_names: Vec<Vec<String>> = vec![Vec::new(), Vec::new()];
        let mut seq = 0usize;
        while shard_names[0].len() < 10 || shard_names[1].len() < 5 {
            let name = format!("sel_{}", seq);
            seq += 1;
            let shard = (fxhash(&name) as usize) & 1;
            let want = if shard == 0 { 10 } else { 5 };
            if shard_names[shard].len() < want {
                shard_names[shard].push(name);
            }
        }
        for names in &shard_names {
            for name in names {
                insert_with_name(&table, name, ts_insert);
            }
        }
        let mut before = std::collections::HashMap::new();
        for names in &shard_names {
            for name in names {
                before.insert(
                    name.clone(),
                    table.get_internal_id(name, ts_delete).unwrap(),
                );
            }
        }
        for name in shard_names[0][3..7].iter() {
            let gid = table.get_internal_id(name, ts_delete).unwrap();
            table.delete_by_internal_id(gid, ts_delete).unwrap();
        }
        let victim = shard_names[1][2].clone();
        let gid = table.get_internal_id(&victim, ts_delete).unwrap();
        table.delete_by_internal_id(gid, ts_delete).unwrap();
        let (removed, mapping, _) = table
            .compact_with_cutoff_collect_mapping(ts_delete)
            .unwrap();
        assert_eq!(
            removed.len(),
            4,
            "only the over-watermark shard is reclaimed; the dense shard keeps its tombstone for lazy reuse"
        );
        assert!(!mapping.is_empty());
        assert_eq!(table.get_internal_id(&victim, ts_delete), None);
        for name in [
            &shard_names[1][0],
            &shard_names[1][1],
            &shard_names[1][3],
            &shard_names[1][4],
        ] {
            assert_eq!(
                table.get_internal_id(name, ts_delete),
                before.get(name).copied(),
                "dense shard survivors must not move"
            );
        }
        for old in mapping.keys() {
            let (shard, _) = table.decode_id(*old);
            assert_eq!(
                shard, 0,
                "mapping must only contain over-watermark shard ids"
            );
        }
        table.verify_invariants().unwrap();
    }

    #[test]
    fn test_estimate_driven_shard_counts() {
        use super::routing::{shards_for_estimate, SINGLE_SHARD_MAX_ESTIMATE};
        assert_eq!(shards_for_estimate(None, 8), 1);
        assert_eq!(shards_for_estimate(Some(0), 8), 1);
        assert_eq!(shards_for_estimate(Some(SINGLE_SHARD_MAX_ESTIMATE), 8), 1);
        assert_eq!(
            shards_for_estimate(Some(SINGLE_SHARD_MAX_ESTIMATE + 1), 8),
            8
        );
        assert_eq!(shards_for_estimate(Some(1_000_000), 12), 16);
        assert_eq!(shards_for_estimate(Some(1_000_000), 300), 256);
    }

    #[test]
    fn test_new_table_defaults_to_single_shard() {
        let table = ShardedVertexTable::new(1, "t".to_string(), test_schema());
        assert_eq!(table.num_shards(), 1);
        let small =
            ShardedVertexTable::with_estimate(1, "t".to_string(), test_schema(), 8, Some(10));
        assert_eq!(small.num_shards(), 1);
        let large =
            ShardedVertexTable::with_estimate(1, "t".to_string(), test_schema(), 8, Some(500_000));
        assert_eq!(large.num_shards(), 8);
    }

    #[test]
    fn test_single_shard_id_tail_bound() {
        let table = ShardedVertexTable::with_estimate(1, "t".to_string(), test_schema(), 8, None);
        assert_eq!(table.num_shards(), 1);
        let ts = TEST_TS;
        let n = 200usize;
        let mut max_id = 0u32;
        for i in 0..n {
            let id = insert_with_name(&table, &format!("tail_{}", i), ts);
            max_id = max_id.max(id);
        }
        assert!(
            (max_id as usize) < n + table.layout().segment_slots() as usize,
            "single-shard tail must not inflate by the shard count: max {max_id} for {n} rows",
        );
    }

    #[test]
    fn test_dry_run_reports_distribution_and_refuses_noop() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        let ts = TEST_TS;
        for i in 0..20 {
            insert_with_name(&table, &format!("d_{}", i), ts);
        }
        let preview = table.dry_run_reshard_to(4).expect("dry run succeeds");
        assert_eq!(preview.new_num_shards, 4);
        assert_eq!(preview.live_rows, 20);
        assert_eq!(preview.per_shard_rows.len(), 4);
        assert_eq!(preview.per_shard_rows.iter().sum::<usize>(), 20);
        let err = table.dry_run_reshard_to(2).unwrap_err().to_string();
        assert!(
            err.contains("no-op"),
            "same-count dry run must refuse: {err}"
        );
        let (rebuilt, mapping) = table.reshard_to(4).expect("rebuild succeeds");
        assert_eq!(mapping.len(), preview.live_rows);
        assert_eq!(rebuilt.approximate_total_count(), preview.live_rows);
    }

    #[test]
    fn test_dry_run_refuses_pending_schema_change() {
        let table = ShardedVertexTable::with_config(1, "t".to_string(), test_schema(), 2);
        insert_with_name(&table, "d_0", TEST_TS);
        table
            .prepare_add_property_staged(StoragePropertyDef {
                name: "nick".to_string(),
                data_type: DataType::String,
                nullable: true,
                default_value: None,
            })
            .unwrap();
        let err = table.dry_run_reshard_to(4).unwrap_err().to_string();
        assert!(
            err.contains("pending schema change"),
            "dry run must fence pending schema: {err}"
        );
        table.abort_pending_schema_change();
        let preview = table
            .dry_run_reshard_to(4)
            .expect("dry run succeeds after abort");
        assert_eq!(preview.live_rows, 1);
    }
}
