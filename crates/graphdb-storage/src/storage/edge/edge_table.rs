//! Edge table module: split into focused sub-modules for maintainability.
//!
//! Organization:
//! - `core`: Core operations (CRUD, properties, queries) for TimeTravelEdgeStore
//! - `simple`: SimpleEdgeStore (single CSR, no history)
//! - `compaction`: Compaction and deletion handling (CSR compaction, property cleanup)
//! - `freeze`: CSR freezing operations (delta to segment conversion)
//! - `segment`: Segment management (CsrSegment, versioning, deletion tracking)
//! - `merge`: Merge strategies (LSM, adaptive, in-place, aggressive)
//! - `mvcc`: MVCC and snapshot management
//! - `snapshot`: Snapshot export and time-travel queries
//! - `persistence`: Serialization (flush/load)
//! - `stats`: Statistics structures (metrics, observability)

pub mod compaction;
pub mod core;
pub mod free_space;
pub mod freeze;
pub mod merge;
pub mod mvcc;
pub mod page_state;
pub mod persistence;
pub mod residency;
pub mod segment;
pub mod segment_eviction;
pub mod snapshot;
pub mod stats;

// Re-export commonly used types
pub use compaction::CompactionMode;
pub use core::UpdateEdgePropertyByOffsetParams;
pub use segment::CsrSegment;
pub use snapshot::ExportedEdgeSnapshot;
pub use stats::{MergeMetrics, MergeMetricsResult, MergeStats};

// Re-export from parent
pub use super::{CsrBase, CsrVariant, Nbr};

use crate::core::types::CompactConfig;
use crate::core::types::{EdgeId, Timestamp};
use crate::core::{StorageError, StorageResult};
use crate::storage::edge::edge_table::core::EdgeTableConfig;
use crate::storage::persistence::write_header_to;
use std::fmt;
use std::time::Instant;

/// Edge store with full MVCC + freeze/merge/segment.
pub struct EdgeStore(pub core::TimeTravelEdgeStore);

impl fmt::Debug for EdgeStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EdgeStore").field(&self.0).finish()
    }
}

impl fmt::Debug for core::TimeTravelEdgeStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimeTravelEdgeStore")
            .field("label", &self.label)
            .field("label_name", &self.label_name)
            .field("out_csr", &self.out_csr)
            .field("in_csr", &self.in_csr)
            .field("out_segments", &self.out_segments.len())
            .field("in_segments", &self.in_segments.len())
            .field("is_open", &self.is_open)
            .field("next_edge_id", &self.next_edge_id)
            .field("config", &self.config)
            .finish()
    }
}

// ── EdgeStore dispatch methods ──
impl EdgeStore {
    pub fn new(schema: super::EdgeSchema) -> StorageResult<Self> {
        Self::new_with_config(schema, EdgeTableConfig::default())
    }

    pub fn new_with_config(
        schema: super::EdgeSchema,
        config: EdgeTableConfig,
    ) -> StorageResult<Self> {
        Ok(EdgeStore(core::TimeTravelEdgeStore::with_config(
            schema, config,
        )?))
    }

    pub fn needs_background_freeze(&self) -> bool {
        self.0.needs_background_freeze()
    }

    // ── Accessors ──
    pub fn label(&self) -> super::LabelId {
        self.0.label()
    }

    pub fn src_label(&self) -> super::LabelId {
        self.0.src_label()
    }

    pub fn dst_label(&self) -> super::LabelId {
        self.0.dst_label()
    }

    pub fn schema(&self) -> &super::EdgeSchema {
        self.0.schema()
    }

    pub fn schema_mut(&mut self) -> &mut super::EdgeSchema {
        self.0.schema_mut()
    }

    pub fn set_stats_manager(&mut self, stats: std::sync::Arc<crate::core::stats::StatsManager>) {
        self.0.set_stats_manager(stats)
    }

    pub fn version_history_ref(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<crate::storage::schema::LabelVersionHistory>> {
        self.0.version_history_ref()
    }

    // ── CRUD ──
    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, crate::core::Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        self.0.insert_edge(src, dst, rank, property_values, ts)
    }

    pub fn delete_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        self.0.delete_edge(src, dst, rank, ts)
    }

    pub fn delete_edge_by_offset(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        self.0
            .delete_edge_by_offset(src, dst, rank, oe_offset, ie_offset, ts)
    }

    pub fn revert_delete_edge_by_offset(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        self.0
            .revert_delete_edge_by_offset(src, dst, rank, oe_offset, ie_offset, ts)
    }

    pub fn get_edge(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> Option<super::EdgeRecord> {
        self.0.get_edge(src, dst, rank, ts)
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<super::EdgeRecord> {
        self.0.out_edges(src, ts)
    }

    pub fn in_edges(&self, dst: u32, ts: Timestamp) -> Vec<super::EdgeRecord> {
        self.0.in_edges(dst, ts)
    }

    pub fn update_edge_property(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        prop_name: &str,
        value: &crate::core::Value,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        self.0
            .update_edge_property(src, dst, rank, prop_name, value, ts)
    }

    pub fn update_edge_property_by_offset(
        &mut self,
        params: UpdateEdgePropertyByOffsetParams,
    ) -> StorageResult<bool> {
        self.0.update_edge_property_by_offset(params)
    }

    // ── Schema operations ──
    pub fn add_property(
        &mut self,
        name: String,
        data_type: crate::core::DataType,
        nullable: bool,
    ) -> StorageResult<()> {
        self.0.add_property(name, data_type, nullable)
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        self.0.remove_property(name)
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        self.0.rename_property(old_name, new_name)
    }

    pub fn rebuild_schema_change_from_redo(
        &mut self,
        details: crate::storage::schema::ChangeDetails,
    ) -> StorageResult<()> {
        self.0.rebuild_schema_change_from_redo(details)
    }

    // ── Query ──
    pub fn scan(&self, ts: Timestamp) -> Vec<super::EdgeRecord> {
        self.0.scan(ts)
    }

    pub fn edge_count(&self) -> u64 {
        self.0.edge_count()
    }

    pub fn delta_edge_count(&self) -> u64 {
        self.0.delta_edge_count()
    }

    // ── Maintenance ──
    pub fn freeze_csr_only(&mut self, ts: Timestamp) -> usize {
        self.0.freeze_csr_only(ts)
    }

    pub fn compact_and_freeze(
        &mut self,
        ts: Timestamp,
        config: &CompactConfig,
        mode: CompactionMode,
    ) -> usize {
        self.0.compact_and_freeze(ts, config, mode)
    }

    pub fn compact_properties(&mut self, ts: Timestamp) {
        self.0.compact_properties(ts)
    }

    pub fn maybe_compact_for_flush(&mut self, ts: Timestamp, threshold: f32) {
        self.0.maybe_compact_for_flush(ts, threshold)
    }

    pub fn merge_segments_lsm_tiered(&mut self, current_ts: Timestamp) -> usize {
        self.0.merge_segments_lsm_tiered(current_ts)
    }

    pub fn merge_segments_adaptive(
        &mut self,
        current_ts: Timestamp,
        max_segment_age: Timestamp,
        deletion_threshold: f64,
        max_segment_size_bytes: usize,
    ) -> usize {
        self.0.merge_segments_adaptive(
            current_ts,
            max_segment_age,
            deletion_threshold,
            max_segment_size_bytes,
        )
    }

    pub fn merge_stats(&self) -> MergeStats {
        self.0.merge_stats()
    }

    pub fn deletion_stats(&self) -> stats::DeletionStats {
        self.0.deletion_stats()
    }

    pub fn validate_segment_integrity(&self) -> usize {
        self.0.validate_segment_integrity()
    }

    pub fn segment_versions(&self) -> Vec<(usize, u32)> {
        self.0.segment_versions()
    }

    // ── Memory ──
    pub fn memory_size(&self) -> usize {
        self.0.memory_size()
    }

    pub fn used_memory_size(&self) -> usize {
        self.0.used_memory_size()
    }

    // ── Snapshots ──
    pub fn register_snapshot(&mut self, ts: Timestamp) {
        self.0.register_snapshot(ts)
    }

    pub fn unregister_snapshot(&mut self, ts: Timestamp) {
        self.0.unregister_snapshot(ts)
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<ExportedEdgeSnapshot> {
        self.0.export_snapshot(ts)
    }

    // ── Persistence ──
    pub fn flush<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
        compression: crate::storage::compression::CompressionType,
    ) -> StorageResult<()> {
        self.0.flush(path, compression)
    }

    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        self.0.load(path)
    }
}

// ── TimeTravelEdgeStore methods ──
impl core::TimeTravelEdgeStore {
    pub fn register_snapshot(&mut self, ts: Timestamp) {
        self.mvcc.register_active_snapshot(ts);
    }

    pub fn unregister_snapshot(&mut self, ts: Timestamp) {
        self.mvcc.unregister_active_snapshot(ts);
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<ExportedEdgeSnapshot> {
        use snapshot::SnapshotBuilder;
        let out_edges =
            self.collect_edges_for_snapshot_mvcc(&self.out_csr, &self.out_segments, ts)?;
        let in_edges = self.collect_edges_for_snapshot_mvcc(&self.in_csr, &self.in_segments, ts)?;

        let out_csr = SnapshotBuilder::build_csr(out_edges, self.out_csr.vertex_capacity())?;
        let in_csr = SnapshotBuilder::build_csr(in_edges, self.in_csr.vertex_capacity())?;

        Ok(ExportedEdgeSnapshot {
            snapshot_ts: ts,
            label: self.label,
            out_csr,
            in_csr,
            properties: self.properties.clone(),
            schema: self.schema.clone(),
        })
    }

    fn collect_edges_for_snapshot_mvcc(
        &self,
        delta: &CsrVariant,
        segments: &[CsrSegment],
        ts: Timestamp,
    ) -> StorageResult<Vec<(u32, Nbr)>> {
        use snapshot::SnapshotBuilder;

        let mut builder = SnapshotBuilder::new();

        for segment in segments.iter().rev() {
            if segment.create_ts_min > ts {
                continue;
            }
            if segment.deletion_info.all_deleted_before(ts)
                && segment
                    .deletion_info
                    .all_edges_deleted(segment.csr.read().edge_count())
            {
                continue;
            }
            builder.add_segment_edges(segment, ts, &self.mvcc.tombstones);
        }

        let delta_edges: Vec<(u32, Nbr)> = delta
            .iter(ts)
            .map(|(src, nbr)| {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                (src_u32, nbr)
            })
            .collect();
        builder.add_delta_edges(delta_edges, ts, &self.mvcc.tombstones);

        Ok(builder.edges())
    }

    pub fn merge_stats(&self) -> MergeStats {
        MergeStats {
            total_merge_operations: 0,
            total_segments_merged: 0,
            total_edges_merged: 0,
            total_merge_time_ms: 0,
            current_segment_count: self.out_segments.len() + self.in_segments.len(),
            max_segment_count: self.config.max_segments_per_direction * 2,
        }
    }

    pub fn merge_segments_lsm_tiered(&mut self, current_ts: Timestamp) -> usize {
        let start = Instant::now();
        let out_reduced = merge::merge_lsm_tiered_with_free_space(
            &mut self.out_segments,
            current_ts,
            &mut self.out_free_space,
        );
        let in_reduced = merge::merge_lsm_tiered_with_free_space(
            &mut self.in_segments,
            current_ts,
            &mut self.in_free_space,
        );

        let total_reduced = out_reduced + in_reduced;
        if total_reduced > 0 {
            let _duration_ms = start.elapsed().as_millis() as u64;
            self.rebuild_segment_indices();
            self.rebuild_sparse_vertex_indices();
            self.rebuild_current_snapshot();
        }
        total_reduced
    }

    pub fn merge_segments_adaptive(
        &mut self,
        current_ts: Timestamp,
        max_segment_age: Timestamp,
        deletion_threshold: f64,
        max_segment_size_bytes: usize,
    ) -> usize {
        let start = Instant::now();
        let out_reduced = merge::merge_adaptive_with_free_space(
            &mut self.out_segments,
            current_ts,
            max_segment_age,
            deletion_threshold,
            max_segment_size_bytes,
            &mut self.out_free_space,
        );
        let in_reduced = merge::merge_adaptive_with_free_space(
            &mut self.in_segments,
            current_ts,
            max_segment_age,
            deletion_threshold,
            max_segment_size_bytes,
            &mut self.in_free_space,
        );

        let total_reduced = out_reduced + in_reduced;
        if total_reduced > 0 {
            let _duration_ms = start.elapsed().as_millis() as u64;
            self.rebuild_segment_indices();
            self.rebuild_sparse_vertex_indices();
            self.rebuild_current_snapshot();
        }
        total_reduced
    }

    pub fn merge_segments_with_config(
        &mut self,
        time_threshold: Timestamp,
        size_threshold_bytes: usize,
    ) -> MergeMetricsResult {
        let start = Instant::now();
        let segments_before = self.out_segments.len() + self.in_segments.len();

        let out_metrics = merge::merge_in_place_with_free_space(
            &mut self.out_segments,
            time_threshold,
            size_threshold_bytes,
            &mut self.out_free_space,
        );
        let in_metrics = merge::merge_in_place_with_free_space(
            &mut self.in_segments,
            time_threshold,
            size_threshold_bytes,
            &mut self.in_free_space,
        );

        let segments_after = self.out_segments.len() + self.in_segments.len();
        let total_edges = out_metrics.edges_processed + in_metrics.edges_processed;
        let duration_ms = start.elapsed().as_millis() as u64;

        if segments_before != segments_after {
            self.rebuild_segment_indices();
            self.rebuild_sparse_vertex_indices();
            self.rebuild_current_snapshot();
        }

        MergeMetricsResult {
            metrics: MergeMetrics {
                segments_before,
                segments_after,
                edges_merged: total_edges,
                duration_ms,
            },
            segments_reduced: segments_before.saturating_sub(segments_after),
        }
    }

    pub fn merge_segments_with_config_and_deletion_filter(
        &mut self,
        time_threshold: Timestamp,
        size_threshold_bytes: usize,
        min_active_snapshot_ts: Option<Timestamp>,
    ) -> MergeMetricsResult {
        let start = Instant::now();
        let segments_before = self.out_segments.len() + self.in_segments.len();

        if let Some(_min_ts) = min_active_snapshot_ts {
            if self.out_segments.len() > 1 {
                let out_indices: Vec<usize> = (0..self.out_segments.len()).collect();
                merge::merge_selected_segments_with_deletion_filter_with_free_space(
                    &mut self.out_segments,
                    out_indices,
                    Timestamp::MAX,
                    min_active_snapshot_ts,
                    &mut self.out_free_space,
                );
            }
            if self.in_segments.len() > 1 {
                let in_indices: Vec<usize> = (0..self.in_segments.len()).collect();
                merge::merge_selected_segments_with_deletion_filter_with_free_space(
                    &mut self.in_segments,
                    in_indices,
                    Timestamp::MAX,
                    min_active_snapshot_ts,
                    &mut self.in_free_space,
                );
            }
        } else {
            let _ = merge::merge_in_place_with_free_space(
                &mut self.out_segments,
                time_threshold,
                size_threshold_bytes,
                &mut self.out_free_space,
            );
            let _ = merge::merge_in_place_with_free_space(
                &mut self.in_segments,
                time_threshold,
                size_threshold_bytes,
                &mut self.in_free_space,
            );
        }

        let segments_after = self.out_segments.len() + self.in_segments.len();
        let duration_ms = start.elapsed().as_millis() as u64;

        if segments_before != segments_after {
            self.rebuild_segment_indices();
            self.rebuild_sparse_vertex_indices();
            self.rebuild_current_snapshot();
        }

        MergeMetricsResult {
            metrics: MergeMetrics {
                segments_before,
                segments_after,
                edges_merged: 0,
                duration_ms,
            },
            segments_reduced: segments_before.saturating_sub(segments_after),
        }
    }

    pub fn flush<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
        compression: crate::storage::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs;
        let path = path.as_ref();
        fs::create_dir_all(path)?;
        crate::storage::compression::cleanup_shadow_files(path)?;

        let crate::storage::compression::CompressionType::Zstd { level } = compression;
        let page_size = crate::storage::compression::DEFAULT_PAGE_SIZE;

        let mut meta_payload = Vec::new();
        write_header_to(
            &mut meta_payload,
            crate::storage::persistence::section::EDGE_META,
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
            &self.mvcc.tombstones,
            self.mvcc.min_active_snapshot_ts,
        )?;
        persistence::write_pages_to_file(
            &path.join("meta.bin"),
            &meta_payload,
            page_size,
            level,
            1,
        )?;

        let mut out_csr_payload = Vec::new();
        persistence::serialize_csr(
            &self.out_csr,
            &self.out_segments,
            crate::storage::persistence::section::EDGE_OUT_CSR,
            &mut out_csr_payload,
        )?;
        let out_edge_count = self.out_csr.edge_count() as u32;
        persistence::write_pages_to_file(
            &path.join("out_csr.bin"),
            &out_csr_payload,
            page_size,
            level,
            out_edge_count,
        )?;

        let mut in_csr_payload = Vec::new();
        persistence::serialize_csr(
            &self.in_csr,
            &self.in_segments,
            crate::storage::persistence::section::EDGE_IN_CSR,
            &mut in_csr_payload,
        )?;
        let in_edge_count = self.in_csr.edge_count() as u32;
        persistence::write_pages_to_file(
            &path.join("in_csr.bin"),
            &in_csr_payload,
            page_size,
            level,
            in_edge_count,
        )?;

        let mut props_payload = Vec::new();
        persistence::serialize_properties(&self.properties, &mut props_payload)?;
        let edge_count = self.next_edge_id.0 as u32;
        persistence::write_pages_to_file(
            &path.join("properties.bin"),
            &props_payload,
            page_size,
            level,
            edge_count,
        )?;

        Ok(())
    }

    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        use std::io::Read;
        let path = path.as_ref();

        let meta_path = path.join("meta.bin");
        let (meta_data, _meta_rows) = persistence::read_pages_from_file(&meta_path)?;
        let mut meta_cursor = &meta_data[..];
        let mut header_buf = [0u8; crate::storage::persistence::HEADER_SIZE];
        meta_cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = crate::storage::persistence::read_header(&mut slice)?;
            if sid != crate::storage::persistence::section::EDGE_META {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in edge meta: expected {:#06x}, got {:#06x}",
                    crate::storage::persistence::section::EDGE_META,
                    sid
                )));
            }
        }

        let mut version_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut version_bytes)?;
        let version = u32::from_le_bytes(version_bytes);
        if version != 2 {
            return Err(StorageError::deserialize_error(format!(
                "unsupported edge meta version: {}",
                version
            )));
        }

        let (
            label,
            src_label,
            dst_label,
            label_name,
            is_open,
            schema,
            next_edge_id,
            tombstones,
            min_snapshot_ts,
        ) = persistence::load_metadata(&mut meta_cursor)?;

        self.label = label;
        self.src_label = src_label;
        self.dst_label = dst_label;
        self.label_name = label_name;
        self.is_open = is_open;
        self.set_schema(schema);
        self.next_edge_id = next_edge_id;
        self.mvcc.tombstones = tombstones;
        self.mvcc.min_active_snapshot_ts = min_snapshot_ts;
        self.out_free_space.clear();
        self.in_free_space.clear();

        let out_csr_path = path.join("out_csr.bin");
        persistence::load_csr(&out_csr_path, &mut self.out_csr, &mut self.out_segments)?;

        let in_csr_path = path.join("in_csr.bin");
        persistence::load_csr(&in_csr_path, &mut self.in_csr, &mut self.in_segments)?;

        let props_path = path.join("properties.bin");
        self.properties = persistence::load_properties(&props_path)?;

        if self.next_edge_id.0 == 0 {
            let ts = Timestamp::MAX;
            let max_id = self
                .out_csr
                .iter(ts)
                .map(|(_, nbr)| nbr.edge_id.0 + 1)
                .chain(self.out_segments.iter().flat_map(|segment| {
                    let csr = segment.csr.read();
                    csr.iter()
                        .map(|(_, nbr)| nbr.edge_id.0 + 1)
                        .collect::<Vec<_>>()
                }))
                .max()
                .unwrap_or(0);
            self.next_edge_id = EdgeId(max_id);
        }
        self.is_open = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::core::types::DataType;
    use crate::core::Value;
use crate::storage::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
use crate::storage::edge::EdgeSchema;
    use crate::storage::types::StoragePropertyDef;

    fn create_test_schema() -> EdgeSchema {
        EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                DataType::Double,
            )],
            oe_strategy: super::super::EdgeStrategy::Multiple,
            ie_strategy: super::super::EdgeStrategy::Multiple,
            schema_version: 1,
        }
    }

    #[test]
    fn test_freeze_csr_preserves_reads() {
        let schema = create_test_schema();
        let mut table = TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 110)
            .unwrap();

        let before = table.scan(150);
        let frozen = table.freeze_csr_only(150);
        let after = table.scan(150);

        assert_eq!(frozen, 4);
        assert_eq!(table.out_segments.len(), 1);
        assert_eq!(table.in_segments.len(), 1);
        assert_eq!(before.len(), after.len());
        assert!(table.has_edge(0, 1, 0, 150));
        assert!(table.has_edge(0, 2, 0, 150));
    }

    #[test]
    fn test_delete_base_segment_uses_tombstone() {
        let schema = create_test_schema();
        let mut table = TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.freeze_csr_only(150);

        assert!(table.delete_edge(0, 1, 0, 200).unwrap());
        assert!(table.has_edge(0, 1, 0, 150));
        assert!(!table.has_edge(0, 1, 0, 250));
        assert_eq!(table.scan(250).len(), 0);
    }
}
