//! Incremental checkpoint: per-group topology files plus a manifest.
//!
//! Layout of one edge-table directory, version 2:
//! - `meta.bin`: header plus label ids, schema, next edge id and the
//!   authoritative edge timestamps.
//! - `groups_manifest.bin`: group address width plus out/in group counts.
//! - `out_g{gid}.bin` / `in_g{gid}.bin`: one page-compressed payload per
//!   group, each self-validated by its row header on load.
//! - `properties.bin`: property columns plus row visibility.
//!
//! Only groups holding uncheckpointed writes are rewritten; clean groups
//! are skipped. The manifest is written last so a crash between group
//! writes and the manifest write drops the in-flight checkpoint but still
//! loads from the previous manifest. Directories holding the old single-file
//! layout (`out_csr.bin` without a manifest) are rejected explicitly, never
//! converted.

use super::core::EdgeStore;
use super::persistence;
use crate::edge::node_group::{EdgeCheckpointKind, TableShardManifest};
use crate::edge::CsrBase;
use graphdb_core::{StorageError, StorageResult};
use std::path::{Path, PathBuf};

/// Manifest file inside an edge-table directory.
pub const GROUPS_MANIFEST_FILE: &str = "groups_manifest.bin";
/// Legacy single-file topology payloads, rejected when no manifest exists.
pub const LEGACY_OUT_CSR_FILE: &str = "out_csr.bin";
pub const LEGACY_IN_CSR_FILE: &str = "in_csr.bin";

pub fn out_group_file(group: usize) -> String {
    format!("out_g{}.bin", group)
}

pub fn in_group_file(group: usize) -> String {
    format!("in_g{}.bin", group)
}

fn out_group_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(out_group_file(group))
}

fn in_group_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(in_group_file(group))
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(GROUPS_MANIFEST_FILE)
}

/// File size for checkpoint byte accounting. Metrics must never fail a
/// checkpoint, so a missing file reports zero instead of an error.
fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

impl EdgeStore {
    /// Mark property columns dirty. Every operation mutating property state
    /// calls this so checkpoints can skip the property file when clean.
    pub(crate) fn mark_properties_dirty(&mut self) {
        self.properties_dirty = true;
    }

    /// Mark property columns dirty and trace the owning groups.
    ///
    /// Property-only writes leave topology files untouched, so the group
    /// trace records column dirt only and never forces a topology rewrite.
    pub(crate) fn mark_properties_dirty_for_edge(&mut self, src: u32, dst: u32) {
        self.properties_dirty = true;
        self.out_csr.mark_column_updated_for(src);
        self.in_csr.mark_column_updated_for(dst);
    }

    /// Flush dirty state incrementally: metadata always, property columns
    /// only when dirty, topology groups only when dirty or missing.
    ///
    /// Property statistics are refreshed before the property payload is
    /// serialized so they follow the checkpoint. The checkpoint kind is
    /// derived from the dirt before it is cleared: any delete dirt makes a
    /// rebalance, otherwise the flush only lands the memory append layer.
    /// Flushed bytes, elapsed time and authority tombstone totals are
    /// reported to the shared metrics registry when one is set. Returns the
    /// checkpoint kind for engine-side logging.
    pub(crate) fn flush_incremental(
        &mut self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<EdgeCheckpointKind> {
        let started = std::time::Instant::now();
        std::fs::create_dir_all(dir)?;
        crate::compression::cleanup_shadow_files(dir)?;

        let kind = match (
            self.out_csr.checkpoint_kind(),
            self.in_csr.checkpoint_kind(),
        ) {
            (EdgeCheckpointKind::Rebalance, _) | (_, EdgeCheckpointKind::Rebalance) => {
                EdgeCheckpointKind::Rebalance
            }
            _ => EdgeCheckpointKind::AppendOnly,
        };
        let dirty_groups =
            self.out_csr.dirty_group_ids().len() + self.in_csr.dirty_group_ids().len();

        let mut flushed_bytes = 0u64;
        flushed_bytes += self.flush_metadata_file(dir, page_size, level)?;
        flushed_bytes += self.flush_properties_file(dir, page_size, level)?;
        flushed_bytes += self.flush_group_set(
            dir,
            page_size,
            level,
            true,
            crate::persistence::section::EDGE_OUT_CSR,
        )?;
        flushed_bytes += self.flush_group_set(
            dir,
            page_size,
            level,
            false,
            crate::persistence::section::EDGE_IN_CSR,
        )?;
        self.write_manifest(dir)?;
        flushed_bytes += file_bytes(&manifest_path(dir));

        self.properties_dirty = false;
        self.out_csr.clear_all_dirty();
        self.in_csr.clear_all_dirty();
        self.remove_orphan_group_files(dir);
        log::debug!(
            "EdgeTable[{}] checkpoint kind={:?} dirty_groups={} bytes={}",
            self.label,
            kind,
            dirty_groups,
            flushed_bytes
        );
        if let Some(stats) = &self.stats_manager {
            stats.record_incremental_checkpoint(started.elapsed(), flushed_bytes);
            stats.record_checkpoint_strategy_by_name("incremental");
            let tombstones = self.mvcc.tombstone_stats();
            let active_snapshots: u64 = self.mvcc.active_snapshots.values().sum::<usize>() as u64;
            let saturate = |ts: Option<u64>| ts.map(|v| u32::try_from(v).unwrap_or(u32::MAX));
            stats.record_tombstone_stats(
                tombstones.count as u64,
                tombstones.memory_bytes as u64,
                saturate(tombstones.oldest_delete_ts),
                saturate(tombstones.newest_delete_ts),
                active_snapshots,
            );
        }
        Ok(kind)
    }

    fn flush_metadata_file(&self, dir: &Path, page_size: usize, level: i32) -> StorageResult<u64> {
        // Meta stays a full rewrite: edge timestamps are a single global map
        // with no column sharding yet, so any timestamp change needs the whole
        // table. Sharded or delta meta is future work once timestamps split
        // by group.
        let mut meta_payload = Vec::new();
        crate::persistence::write_header_to(
            &mut meta_payload,
            crate::persistence::section::EDGE_META,
        )
        .map_err(|e| StorageError::io_error(format!("Failed to write edge meta header: {}", e)))?;
        persistence::flush_metadata(
            &mut meta_payload,
            self.label,
            self.src_label,
            self.dst_label,
            &self.label_name,
            self.is_open,
            &self.schema,
            self.next_edge_id,
            &self.mvcc.edge_timestamps,
        )?;
        let path = dir.join("meta.bin");
        persistence::write_pages_to_file(&path, &meta_payload, page_size, level, 1)?;
        Ok(file_bytes(&path))
    }

    fn flush_properties_file(
        &mut self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<u64> {
        let path = dir.join("properties.bin");
        if !self.properties_dirty && path.exists() {
            return Ok(0);
        }
        // Column-level follow-up: only dirty columns recompute stats, clean
        // columns keep persisted values. The file itself is still a full
        // rewrite; per-column files are future work once properties split.
        self.properties.refresh_column_stats();
        let mut props_payload = Vec::new();
        persistence::serialize_csr_properties(&self.properties, &mut props_payload)?;
        let edge_count = self.properties.row_count() as u32;
        persistence::write_pages_to_file(&path, &props_payload, page_size, level, edge_count)?;
        self.properties.clear_dirty_columns();
        Ok(file_bytes(&path))
    }

    /// Write one direction. `outgoing` selects the out shard set.
    ///
    /// Returns the bytes written for groups actually flushed; clean groups
    /// whose files are skipped contribute zero.
    fn flush_group_set(
        &self,
        dir: &Path,
        page_size: usize,
        level: i32,
        outgoing: bool,
        section_id: u32,
    ) -> StorageResult<u64> {
        let shards = if outgoing {
            &self.out_csr
        } else {
            &self.in_csr
        };
        let mut written = 0u64;
        for gid in 0..shards.group_count() {
            let path = if outgoing {
                out_group_path(dir, gid)
            } else {
                in_group_path(dir, gid)
            };
            if !shards.needs_checkpoint(gid) && path.exists() {
                continue;
            }
            let Some(variant) = shards.group_variant(gid) else {
                continue;
            };
            let mut payload = Vec::new();
            persistence::serialize_csr(variant, section_id, &mut payload)?;
            persistence::write_pages_to_file(
                &path,
                &payload,
                page_size,
                level,
                variant.edge_count() as u32,
            )?;
            written += file_bytes(&path);
        }
        Ok(written)
    }

    fn write_manifest(&self, dir: &Path) -> StorageResult<()> {
        let manifest = TableShardManifest {
            group_bits: self.config.node_group_bits,
            out_groups: self.out_csr.group_count() as u32,
            in_groups: self.in_csr.group_count() as u32,
        };
        crate::compression::write_shadow_file(manifest_path(dir), &manifest.encode())
    }

    fn remove_orphan_group_files(&self, dir: &Path) {
        let mut gid = self.out_csr.group_count();
        loop {
            let path = out_group_path(dir, gid);
            if !path.exists() || std::fs::remove_file(&path).is_err() {
                break;
            }
            gid += 1;
        }
        let mut gid = self.in_csr.group_count();
        loop {
            let path = in_group_path(dir, gid);
            if !path.exists() || std::fs::remove_file(&path).is_err() {
                break;
            }
            gid += 1;
        }
    }

    /// Load an incremental checkpoint. Directories in the old single-file
    /// layout are rejected explicitly.
    pub(crate) fn load_incremental(&mut self, dir: &Path) -> StorageResult<()> {
        let manifest_file = manifest_path(dir);
        if !manifest_file.exists() {
            if dir.join(LEGACY_OUT_CSR_FILE).exists() || dir.join(LEGACY_IN_CSR_FILE).exists() {
                return Err(StorageError::deserialize_error(
                    "legacy single-file edge layout without a group manifest is not supported"
                        .to_string(),
                ));
            }
            return Err(StorageError::io_error(format!(
                "missing group manifest: {}",
                manifest_file.display()
            )));
        }
        let manifest_bytes = std::fs::read(&manifest_file)
            .map_err(|e| StorageError::io_error(format!("Failed to read group manifest: {}", e)))?;
        let manifest = TableShardManifest::decode(&manifest_bytes)?;
        if manifest.group_bits != self.config.node_group_bits {
            return Err(StorageError::deserialize_error(format!(
                "group layout mismatch: file uses {} address bits, table uses {}",
                manifest.group_bits, self.config.node_group_bits
            )));
        }

        self.load_metadata_file(dir)?;
        self.out_csr.resize_groups(manifest.out_groups as usize)?;
        self.in_csr.resize_groups(manifest.in_groups as usize)?;
        self.load_group_set(dir, true, manifest.out_groups as usize)?;
        self.load_group_set(dir, false, manifest.in_groups as usize)?;
        self.load_properties_file(dir)?;

        if self.next_edge_id.0 == 0 {
            let max_id = self
                .out_csr
                .iter_all()
                .map(|(_, nbr)| nbr.edge_id.0 + 1)
                .max()
                .unwrap_or(0);
            self.next_edge_id = graphdb_core::types::EdgeId(max_id);
        }
        let (orphan_mappings, orphan_csr_rows) = self.loaded_copy_mismatches();
        let live_orphans = self.live_authority_orphans();
        if orphan_mappings + orphan_csr_rows + live_orphans > 0 {
            return Err(crate::StorageError::db_error(format!(
                "edge table {} loaded with copy mismatches: \
                 orphan property mappings={}, orphan CSR rows={}, \
                 live authority orphans={}",
                self.label_name,
                orphan_mappings,
                orphan_csr_rows,
                live_orphans,
            )));
        }
        self.properties_dirty = false;
        self.out_csr.clear_all_dirty();
        self.in_csr.clear_all_dirty();
        // Pending staged schema changes are memory-only: a reload after any
        // crash is equivalent to aborting them, because the property store is
        // rebuilt from the published schema below.
        self.pending_add_column = None;
        self.pending_drop_column = None;
        self.is_open = true;
        Ok(())
    }

    fn load_metadata_file(&mut self, dir: &Path) -> StorageResult<()> {
        use std::io::Read;
        let meta_path = dir.join("meta.bin");
        let (meta_data, _meta_rows) = persistence::read_pages_from_file(&meta_path)?;
        let mut meta_cursor = &meta_data[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        meta_cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = crate::persistence::read_header(&mut slice)?;
            if sid != crate::persistence::section::EDGE_META {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in edge meta: expected {:#06x}, got {:#06x}",
                    crate::persistence::section::EDGE_META,
                    sid
                )));
            }
        }

        let mut version_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut version_bytes)?;
        let version = u32::from_le_bytes(version_bytes);
        if version != persistence::EDGE_META_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported edge meta version: {}",
                version
            )));
        }

        let meta = persistence::load_metadata(&mut meta_cursor)?;
        self.label = meta.label;
        self.src_label = meta.src_label;
        self.dst_label = meta.dst_label;
        self.label_name = meta.label_name;
        self.is_open = meta.is_open;
        self.set_schema(meta.schema);
        self.next_edge_id = meta.next_edge_id;
        self.mvcc.edge_timestamps = meta.edge_timestamps;
        self.mvcc.min_active_snapshot_ts = graphdb_core::types::Timestamp::MAX;
        self.mvcc.active_snapshots.clear();
        Ok(())
    }

    fn load_group_set(&mut self, dir: &Path, outgoing: bool, count: usize) -> StorageResult<()> {
        for gid in 0..count {
            let path = if outgoing {
                out_group_path(dir, gid)
            } else {
                in_group_path(dir, gid)
            };
            let expected = if outgoing {
                crate::persistence::section::EDGE_OUT_CSR
            } else {
                crate::persistence::section::EDGE_IN_CSR
            };
            let shards = if outgoing {
                &mut self.out_csr
            } else {
                &mut self.in_csr
            };
            let variant = shards.group_variant_mut(gid).ok_or_else(|| {
                StorageError::deserialize_error(format!("group {} missing on load", gid))
            })?;
            persistence::load_csr(&path, variant, expected)?;
            shards.clear_group_dirty(gid);
        }
        Ok(())
    }

    fn load_properties_file(&mut self, dir: &Path) -> StorageResult<()> {
        use crate::edge::property_schema::PropertySchema;
        let props_path = dir.join("properties.bin");
        self.properties = {
            let prop_schemas: Vec<PropertySchema> = self
                .schema
                .properties
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    PropertySchema::new(p.name.clone(), i as i32, p.data_type.clone())
                        .nullable(p.nullable)
                        .with_default_value(p.default_value.clone())
                })
                .collect();
            persistence::load_csr_properties(&props_path, prop_schemas)?
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::types::StoragePropertyDef;
    use graphdb_core::Value;

    fn make_table() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "weight".to_string(),
                data_type: graphdb_core::types::DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn clean_groups_are_skipped_on_flush() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let group_zero = dir.path().join(out_group_file(0));
        assert!(group_zero.exists());
        let stamp = group_zero.metadata().unwrap().modified().unwrap();

        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("second flush should succeed");
        assert_eq!(group_zero.metadata().unwrap().modified().unwrap(), stamp);
    }

    #[test]
    fn dirty_group_roundtrip_preserves_cross_group_edges() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table.insert_edge(5000, 6000, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert!(dir.path().join(out_group_file(1)).exists());

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.has_edge(5000, 6000, 0, 200));
        assert_eq!(loaded.edge_count(), 2);
        assert!(loaded.out_csr.dirty_group_ids().is_empty());
        assert!(loaded.in_csr.dirty_group_ids().is_empty());
    }

    #[test]
    fn legacy_layout_without_manifest_is_rejected() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        std::fs::create_dir_all(dir.path()).unwrap();
        let mut payload = Vec::new();
        let variant = table.out_csr.group_variant(0).unwrap().clone();
        persistence::serialize_csr(
            &variant,
            crate::persistence::section::EDGE_OUT_CSR,
            &mut payload,
        )
        .unwrap();
        persistence::write_pages_to_file(
            &dir.path().join(LEGACY_OUT_CSR_FILE),
            &payload,
            crate::compression::DEFAULT_PAGE_SIZE,
            3,
            1,
        )
        .unwrap();

        let mut loaded = make_table();
        let err = loaded
            .load(dir.path())
            .expect_err("legacy layout must be rejected");
        assert!(err.to_string().contains("legacy single-file"));
    }

    #[test]
    fn properties_file_skipped_when_clean() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let props = dir.path().join("properties.bin");
        let stamp = props.metadata().unwrap().modified().unwrap();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("second flush should succeed");
        assert_eq!(props.metadata().unwrap().modified().unwrap(), stamp);
    }

    #[test]
    fn flush_records_incremental_checkpoint_metrics() {
        use graphdb_metrics::{MetricType, StatsManager};

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let stats = std::sync::Arc::new(StatsManager::new());
        table.set_stats_manager(stats.clone());
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let bytes = stats
            .get_value(MetricType::CheckpointIncrementalBytesFlushed)
            .unwrap_or(0);
        assert!(bytes > 0, "flushed bytes should be recorded");
        assert_eq!(
            stats.get_value(MetricType::CheckpointStrategyIncremental),
            Some(1)
        );
    }

    #[test]
    fn flush_without_metrics_registry_behaves_the_same() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush without a registry should succeed");
        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.has_edge(0, 1, 0, 200));
    }

    #[test]
    fn unpublished_column_is_dropped_on_reload() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        // Fill the physical column but never publish: a crash here must be
        // equivalent to aborting the staged change.
        table
            .prepare_add_property("score".to_string(), graphdb_core::DataType::Int, true, None)
            .unwrap();
        table.fill_pending_add_property().unwrap();
        assert!(table.properties.has_property("score"));
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(!loaded.properties.has_property("score"));
        assert!(!loaded.schema.properties.iter().any(|p| p.name == "score"));
        assert!(loaded.has_edge(0, 1, 0, 200));
        assert!(loaded.pending_add_column().is_none());
    }

    #[test]
    fn published_column_survives_reload_with_stats() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(3.0))], 100)
            .unwrap();
        table
            .prepare_add_property(
                "score".to_string(),
                graphdb_core::DataType::Int,
                true,
                Some(Value::Int(7)),
            )
            .unwrap();
        table.fill_pending_add_property().unwrap();
        table.publish_pending_add_property().unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        // A fresh table only knows the published schema when loading.
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![
                StoragePropertyDef::new(
                    "weight".to_string(),
                    graphdb_core::types::DataType::Double,
                ),
                StoragePropertyDef::new("score".to_string(), graphdb_core::types::DataType::Int),
            ],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut loaded =
            EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.properties.has_property("score"));
        let snapshot = loaded
            .column_stats_snapshot("weight")
            .expect("flushed stats should be queryable");
        assert_eq!(snapshot.row_count, 1);
        assert_eq!(snapshot.min_value, Some(Value::Double(3.0)));
        assert_eq!(snapshot.max_value, Some(Value::Double(3.0)));
    }

    #[test]
    fn encoded_values_survive_reload_with_encoding() {
        let mut table = make_table();
        for i in 0..20 {
            table
                .insert_edge(
                    i,
                    i + 100,
                    0,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    100,
                )
                .unwrap();
        }
        let encoded = table.encode_property_columns();
        assert!(encoded > 0);
        let before = table.properties.column_encoding_type("weight");
        assert!(before.is_some_and(|enc| enc != crate::encoding::EncodingType::None));
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert_eq!(loaded.properties.column_encoding_type("weight"), before);
        let record = loaded.get_edge(3, 103, 0, 200).expect("edge survives");
        assert!(record
            .properties
            .iter()
            .any(|(k, v)| k == "weight" && *v == Value::Double(3.0)));
        let snapshot = loaded
            .column_stats_snapshot("weight")
            .expect("flushed stats should be queryable");
        assert_eq!(snapshot.row_count, 20);
        assert!(snapshot.null_count.is_some());
    }

    #[test]
    fn legacy_properties_version_is_rejected() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let mut payload = table.properties.dump();
        assert!(!payload.is_empty());
        payload[0] = 2;
        let mut reloaded = make_table();
        assert!(reloaded.properties.load(&payload).is_err());
    }

    #[test]
    fn stable_column_ids_survive_drop_and_reload() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .add_property("score".to_string(), graphdb_core::DataType::Int, true)
            .expect("add score should succeed");
        let score_id = table
            .properties
            .get_property_id("score")
            .expect("score id should exist");
        table
            .remove_property("weight")
            .expect("drop should succeed");
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "score".to_string(),
                graphdb_core::types::DataType::Int,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut loaded =
            EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
        loaded.load(dir.path()).expect("load should succeed");
        assert_eq!(loaded.properties.get_property_id("score"), Some(score_id));
        loaded
            .add_property("extra".to_string(), graphdb_core::DataType::Int, true)
            .expect("add extra should succeed");
        let extra_id = loaded
            .properties
            .get_property_id("extra")
            .expect("extra id should exist");
        assert!(extra_id != score_id);
        assert!(extra_id > score_id);
    }

    #[test]
    fn insert_only_flush_reports_append_only() {
        use crate::edge::EdgeCheckpointKind;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
    }

    #[test]
    fn delete_flush_reports_rebalance() {
        use crate::edge::EdgeCheckpointKind;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        table.delete_edge(0, 1, 0, 200).unwrap();
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::Rebalance);
    }

    #[test]
    fn property_only_update_skips_topology_rewrite() {
        use crate::edge::EdgeCheckpointKind;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let group_zero = dir.path().join(out_group_file(0));
        let stamp = group_zero.metadata().unwrap().modified().unwrap();

        table
            .update_edge_property(0, 1, 0, "weight", &Value::Double(9.0), 200)
            .expect("property update should succeed");
        assert!(!table.out_csr.column_dirty_group_ids().is_empty());
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::AppendOnly);
        assert_eq!(group_zero.metadata().unwrap().modified().unwrap(), stamp);
    }

    #[test]
    fn remap_forces_rebalance_checkpoint() {
        use crate::edge::EdgeCheckpointKind;
        use std::collections::HashMap;

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table.insert_edge(5000, 6000, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert!(table.out_csr.dirty_group_ids().is_empty());

        let src_mapping: HashMap<u32, u32> = [(5000u32, 2u32)].into_iter().collect();
        let dst_mapping: HashMap<u32, u32> = [(6000u32, 3u32)].into_iter().collect();
        table
            .remap_vertex_ids(Some(&src_mapping), Some(&dst_mapping))
            .expect("remap should succeed");
        assert!(!table.out_csr.dirty_group_ids().is_empty());
        assert!(table.has_edge(2, 3, 0, 200));
        let kind = table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(kind, EdgeCheckpointKind::Rebalance);
    }

    #[test]
    fn flush_reports_tombstone_totals_to_registry() {
        use graphdb_metrics::{MetricType, StatsManager};

        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        table.delete_edge(0, 1, 0, 200).unwrap();
        let stats = std::sync::Arc::new(StatsManager::new());
        table.set_stats_manager(stats.clone());
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        assert_eq!(stats.get_value(MetricType::TombstoneCount), Some(1));
        assert!(
            stats
                .get_value(MetricType::TombstoneMemoryBytes)
                .unwrap_or(0)
                > 0
        );
    }
}
