//! Edge table module: split into focused sub-modules for maintainability.
//!
//! Organization:
//! - `core`: Core operations (CRUD, properties, queries)
//! - `compaction`: Compaction and deletion handling (CSR compaction, property cleanup)
//! - `freeze`: CSR freezing operations (delta to segment conversion)
//! - `segment`: Segment management (CsrSegment, versioning, deletion tracking)
//! - `merge`: Merge strategies (LSM, adaptive, in-place, aggressive)
//! - `mvcc`: MVCC and snapshot management
//! - `snapshot`: Snapshot export and time-travel queries
//! - `persistence`: Serialization (flush/load)
//! - `stats`: Statistics structures (metrics, observability)

pub mod core;
pub mod compaction;
pub mod freeze;
pub mod segment;
pub mod merge;
pub mod mvcc;
pub mod snapshot;
pub mod persistence;
pub mod stats;

// Re-export public types for backward compatibility
pub use core::{
    EdgeTableCore as EdgeTable, EdgeTableCore, EdgeTableConfig, EdgeTableScanIterator,
    UpdateEdgePropertyByOffsetParams,
};
pub use segment::{CsrSegment, DeletionInfo, SegmentVersion, SEPARATE_EDGE_ID_STORAGE_THRESHOLD};
pub use mvcc::MVCCManager;
pub use snapshot::{ExportedEdgeSnapshot, SnapshotBuilder};
pub use stats::{TombstoneStats, DeletionStats, MergeStats, MergeMetrics, MergeMetricsResult};
pub use compaction::CompactionMode;

// Re-export from parent
pub use super::{
    Csr, CsrVariant, Nbr, ImmutableNbr, EdgeSchema, EdgeRecord, EdgeStrategy, CsrBase, MutableCsrTrait,
};

use crate::core::types::{Timestamp, EdgeId, CompactConfig, LabelId, VertexId};
use crate::core::{StorageResult, StorageError};
use std::time::Instant;
use std::sync::Arc;
use crate::storage::persistence::write_header_to;
impl EdgeTable {
    /// Export a read-only snapshot of this edge table at the given timestamp.
    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<ExportedEdgeSnapshot> {
        let out_edges = self.collect_edges_for_snapshot_mvcc(&self.out_csr, &self.out_segments, ts)?;
        let in_edges = self.collect_edges_for_snapshot_mvcc(&self.in_csr, &self.in_segments, ts)?;

        let out_csr = Self::build_csr_from_edges(out_edges, self.out_csr.vertex_capacity())?;
        let in_csr = Self::build_csr_from_edges(in_edges, self.in_csr.vertex_capacity())?;

        Ok(ExportedEdgeSnapshot {
            snapshot_ts: ts,
            label: self.label,
            out_csr,
            in_csr,
            properties: self.properties.clone(),
            schema: self.schema.clone(),
        })
    }

    /// Collect edges visible at timestamp from delta and segments with MVCC filtering.
    fn collect_edges_for_snapshot_mvcc(
        &self,
        delta: &CsrVariant,
        segments: &[CsrSegment],
        ts: Timestamp,
    ) -> StorageResult<Vec<(u32, Nbr)>> {
        use std::collections::HashMap;

        let mut edge_map: HashMap<(u32, EdgeId), (u32, Nbr)> = HashMap::new();

        for segment in segments.iter().rev() {
            if segment.create_ts_min > ts {
                continue;
            }

            if segment.deletion_info.all_deleted_before(ts)
                && segment.deletion_info.all_edges_deleted(segment.csr.edge_count()) {
                continue;
            }

            let mut edge_position = 0usize;
            for (src, immutable_nbr) in segment.csr.iter() {
                let edge_id = segment.recover_edge_id(immutable_nbr, edge_position);
                edge_position += 1;

                if immutable_nbr.timestamp > ts {
                    continue;
                }

                if let Some(&delete_ts) = self.mvcc.tombstones.get(&edge_id) {
                    if delete_ts <= ts {
                        continue;
                    }
                }

                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                let nbr = Nbr::new(
                    immutable_nbr.neighbor,
                    edge_id,
                    immutable_nbr.prop_offset,
                    immutable_nbr.timestamp,
                );
                edge_map.insert((src_u32, edge_id), (src_u32, nbr));
            }
        }

        for (src, nbr) in delta.iter(ts) {
            let src_u32 = src.as_int64().unwrap_or(0) as u32;

            if let Some(&delete_ts) = self.mvcc.tombstones.get(&nbr.edge_id) {
                if delete_ts <= ts {
                    continue;
                }
            }

            edge_map.insert((src_u32, nbr.edge_id), (src_u32, nbr));
        }

        let mut edges: Vec<_> = edge_map.into_values().collect();
        edges.sort_by_key(|(src, _)| *src);

        Ok(edges)
    }

    /// Build a CSR from a list of edges.
    fn build_csr_from_edges(
        edges: Vec<(u32, Nbr)>,
        vertex_capacity: usize,
    ) -> StorageResult<Csr> {
        Ok(Csr::from_nbr_entries(&edges, vertex_capacity))
    }

    /// Get current merge statistics
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

    /// Merge CSR segments with LSM tiered strategy
    pub fn merge_segments_lsm_tiered(&mut self, current_ts: Timestamp) -> usize {
        let start = Instant::now();
        let out_reduced = merge::merge_lsm_tiered(&mut self.out_segments, current_ts);
        let in_reduced = merge::merge_lsm_tiered(&mut self.in_segments, current_ts);

        let total_reduced = out_reduced + in_reduced;
        if total_reduced > 0 {
            let duration_ms = start.elapsed().as_millis() as u64;
            self.rebuild_segment_indices();
            // Record metrics if needed
        }

        total_reduced
    }

    /// Merge CSR segments with adaptive strategy
    pub fn merge_segments_adaptive(&mut self, current_ts: Timestamp, max_segment_age: Timestamp) -> usize {
        let start = Instant::now();
        let out_reduced = merge::merge_adaptive(&mut self.out_segments, current_ts, max_segment_age);
        let in_reduced = merge::merge_adaptive(&mut self.in_segments, current_ts, max_segment_age);

        let total_reduced = out_reduced + in_reduced;
        if total_reduced > 0 {
            let duration_ms = start.elapsed().as_millis() as u64;
            self.rebuild_segment_indices();
        }

        total_reduced
    }

    /// Merge segments with time and size thresholds
    pub fn merge_segments_with_config(&mut self, time_threshold: Timestamp, size_threshold_bytes: usize) -> MergeMetricsResult {
        let start = Instant::now();
        let segments_before = self.out_segments.len() + self.in_segments.len();

        let out_metrics = merge::merge_in_place(&mut self.out_segments, time_threshold, size_threshold_bytes);
        let in_metrics = merge::merge_in_place(&mut self.in_segments, time_threshold, size_threshold_bytes);

        let segments_after = self.out_segments.len() + self.in_segments.len();
        let total_edges = out_metrics.edges_processed + in_metrics.edges_processed;
        let duration_ms = start.elapsed().as_millis() as u64;

        if segments_before != segments_after {
            self.rebuild_segment_indices();
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

    /// Merge segments with physical deletion of tombstoned edges.
    ///
    /// Combines segment reorganization with physical deletion of edges that have been
    /// deleted before min_active_snapshot_ts. This ensures:
    /// 1. Segments are reorganized for better locality
    /// 2. Edges deleted before the minimum active snapshot are physically removed
    /// 3. MVCC visibility is maintained for all active snapshots
    ///
    /// # Parameters
    ///
    /// - `time_threshold`: Merge segments if time gap <= this value
    /// - `size_threshold_bytes`: Merge segments if combined size <= this value
    /// - `min_active_snapshot_ts`: Edges deleted before this timestamp can be physically removed
    ///
    /// # Returns
    ///
    /// MergeMetricsResult containing segment counts before/after and reduction count.
    pub fn merge_segments_with_config_and_deletion_filter(
        &mut self,
        time_threshold: Timestamp,
        size_threshold_bytes: usize,
        min_active_snapshot_ts: Option<Timestamp>,
    ) -> MergeMetricsResult {
        let start = Instant::now();
        let segments_before = self.out_segments.len() + self.in_segments.len();

        if let Some(_min_ts) = min_active_snapshot_ts {
            // Merge out segments with deletion filter
            if self.out_segments.len() > 1 {
                let out_indices: Vec<usize> = (0..self.out_segments.len()).collect();
                merge::merge_selected_segments_with_deletion_filter(
                    &mut self.out_segments,
                    out_indices,
                    u32::MAX,  // Use max timestamp to include all edges in CSR iteration
                    min_active_snapshot_ts,
                );
            }

            // Merge in segments with deletion filter
            if self.in_segments.len() > 1 {
                let in_indices: Vec<usize> = (0..self.in_segments.len()).collect();
                merge::merge_selected_segments_with_deletion_filter(
                    &mut self.in_segments,
                    in_indices,
                    u32::MAX,
                    min_active_snapshot_ts,
                );
            }
        } else {
            // If no min_active_snapshot_ts, fall back to standard merge
            let _ = merge::merge_in_place(&mut self.out_segments, time_threshold, size_threshold_bytes);
            let _ = merge::merge_in_place(&mut self.in_segments, time_threshold, size_threshold_bytes);
        }

        let segments_after = self.out_segments.len() + self.in_segments.len();
        let duration_ms = start.elapsed().as_millis() as u64;

        if segments_before != segments_after {
            self.rebuild_segment_indices();
        }

        MergeMetricsResult {
            metrics: MergeMetrics {
                segments_before,
                segments_after,
                edges_merged: 0,  // Complex to compute from merged results
                duration_ms,
            },
            segments_reduced: segments_before.saturating_sub(segments_after),
        }
    }

    /// Persistence: flush to disk
    pub fn flush<P: AsRef<std::path::Path>>(
        &self,
        path: P,
        compression: crate::storage::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs;

        let path = path.as_ref();
        fs::create_dir_all(path)?;

        let meta_path = path.join("meta.bin");
        let mut meta_file = std::fs::File::create(&meta_path)?;
        write_header_to(&mut meta_file, crate::storage::persistence::section::EDGE_META)
            .map_err(|e| StorageError::io_error(format!("Failed to write edge meta header: {}", e)))?;

        persistence::flush_metadata(
            &mut meta_file,
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

        drop(meta_file);
        crate::storage::compression::compress_file_inplace(&meta_path, compression)?;

        let out_csr_path = path.join("out_csr.bin");
        persistence::flush_csr(
            &self.out_csr,
            &self.out_segments,
            &out_csr_path,
            crate::storage::persistence::section::EDGE_OUT_CSR,
        )?;
        crate::storage::compression::compress_file_inplace(&out_csr_path, compression)?;

        let in_csr_path = path.join("in_csr.bin");
        persistence::flush_csr(
            &self.in_csr,
            &self.in_segments,
            &in_csr_path,
            crate::storage::persistence::section::EDGE_IN_CSR,
        )?;
        crate::storage::compression::compress_file_inplace(&in_csr_path, compression)?;

        let props_path = path.join("properties.bin");
        persistence::flush_properties(&self.properties, &props_path)?;
        crate::storage::compression::compress_file_inplace(&props_path, compression)?;

        Ok(())
    }

    /// Persistence: load from disk
    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        use std::io::Read;

        let path = path.as_ref();

        let meta_path = path.join("meta.bin");
        let meta_data = crate::storage::compression::read_decompressed(&meta_path)?;
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

        let (label, src_label, dst_label, label_name, is_open, schema, next_edge_id, tombstones, min_snapshot_ts) =
            persistence::load_metadata(&mut meta_cursor)?;

        self.label = label;
        self.src_label = src_label;
        self.dst_label = dst_label;
        self.label_name = label_name;
        self.is_open = is_open;
        // Rebuild property index cache from loaded schema
        self.property_index_cache.clear();
        for (idx, prop) in schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
        self.schema = schema;
        self.next_edge_id = next_edge_id;
        self.mvcc.tombstones = tombstones;
        self.mvcc.min_active_snapshot_ts = min_snapshot_ts;

        let out_csr_path = path.join("out_csr.bin");
        persistence::load_csr(&out_csr_path, &mut self.out_csr, &mut self.out_segments)?;

        let in_csr_path = path.join("in_csr.bin");
        persistence::load_csr(&in_csr_path, &mut self.in_csr, &mut self.in_segments)?;

        let props_path = path.join("properties.bin");
        self.properties = persistence::load_properties(&props_path)?;

        if self.next_edge_id.0 == 0 {
            let ts = u32::MAX;
            let max_id = self
                .out_csr
                .iter(ts)
                .map(|(_, nbr)| nbr.edge_id.0 + 1)
                .chain(
                    self.out_segments
                        .iter()
                        .flat_map(|segment| segment.csr.iter().map(|(_, nbr)| nbr.edge_id.0 + 1)),
                )
                .max()
                .unwrap_or(0);
            self.next_edge_id = EdgeId(max_id);
        }
        self.is_open = true;
        Ok(())
    }
}

#[cfg(test)]
mod core_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{VertexId, DataType};
    use crate::core::Value;
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
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        }
    }

    #[test]
    fn test_freeze_csr_preserves_reads() {
        let schema = create_test_schema();
        let mut table = EdgeTable::new(schema).unwrap();

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
        let mut table = EdgeTable::new(schema).unwrap();

        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.freeze_csr_only(150);

        assert!(table.delete_edge(0, 1, 0, 200).unwrap());
        assert!(table.has_edge(0, 1, 0, 150));
        assert!(!table.has_edge(0, 1, 0, 250));
        assert_eq!(table.scan(250).len(), 0);
    }
}
