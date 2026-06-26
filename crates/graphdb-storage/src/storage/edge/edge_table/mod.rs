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

pub mod core;
pub mod compaction;
pub mod freeze;
pub mod segment;
pub mod merge;
pub mod mvcc;
pub mod snapshot;
pub mod persistence;
pub mod stats;
pub mod simple;

// Re-export from parent
pub use super::{
    CsrVariant, Nbr, CsrBase,
};

use crate::core::types::{Timestamp, EdgeId};
use crate::core::{StorageResult, StorageError};
use std::time::Instant;
use std::fmt;
use crate::storage::persistence::write_header_to;
use crate::core::types::CompactConfig;

/// TimeTravel variant: multi-segment CSR with freeze/merge/MVCC (full history).
/// Simple variant: single-segment CSR, no history.
pub enum EdgeStore {
    TimeTravel(core::TimeTravelEdgeStore),
    Simple(simple::SimpleEdgeStore),
}

impl fmt::Debug for EdgeStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EdgeStore::TimeTravel(s) => f.debug_tuple("TimeTravel").field(s).finish(),
            EdgeStore::Simple(s) => f.debug_tuple("Simple").field(s).finish(),
        }
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

impl fmt::Debug for simple::SimpleEdgeStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimpleEdgeStore")
            .field("label", &self.label)
            .field("label_name", &self.label_name)
            .field("out_csr", &self.out_csr)
            .field("in_csr", &self.in_csr)
            .field("is_open", &self.is_open)
            .field("next_edge_id", &self.next_edge_id)
            .finish()
    }
}

// ── EdgeStore dispatch methods ──
impl EdgeStore {
    pub fn new(schema: super::EdgeSchema) -> StorageResult<Self> {
        Ok(EdgeStore::TimeTravel(core::TimeTravelEdgeStore::new(schema)?))
    }

    pub fn with_config(schema: super::EdgeSchema, config: core::EdgeTableConfig) -> StorageResult<Self> {
        Ok(EdgeStore::TimeTravel(core::TimeTravelEdgeStore::with_config(schema, config)?))
    }

    pub fn as_time_travel(&self) -> &core::TimeTravelEdgeStore {
        match self {
            EdgeStore::TimeTravel(s) => s,
            EdgeStore::Simple(_) => panic!("not a TimeTravel variant"),
        }
    }

    pub fn as_time_travel_mut(&mut self) -> &mut core::TimeTravelEdgeStore {
        match self {
            EdgeStore::TimeTravel(s) => s,
            EdgeStore::Simple(_) => panic!("not a TimeTravel variant"),
        }
    }

    // ── Accessors ──
    pub fn label(&self) -> super::LabelId {
        match self {
            EdgeStore::TimeTravel(s) => s.label(),
            EdgeStore::Simple(s) => s.label(),
        }
    }

    pub fn label_name(&self) -> &str {
        match self {
            EdgeStore::TimeTravel(s) => s.label_name(),
            EdgeStore::Simple(s) => s.label_name(),
        }
    }

    pub fn src_label(&self) -> super::LabelId {
        match self {
            EdgeStore::TimeTravel(s) => s.src_label(),
            EdgeStore::Simple(s) => s.src_label(),
        }
    }

    pub fn dst_label(&self) -> super::LabelId {
        match self {
            EdgeStore::TimeTravel(s) => s.dst_label(),
            EdgeStore::Simple(s) => s.dst_label(),
        }
    }

    pub fn schema(&self) -> &super::EdgeSchema {
        match self {
            EdgeStore::TimeTravel(s) => s.schema(),
            EdgeStore::Simple(s) => s.schema(),
        }
    }

    pub fn schema_mut(&mut self) -> &mut super::EdgeSchema {
        match self {
            EdgeStore::TimeTravel(s) => s.schema_mut(),
            EdgeStore::Simple(s) => s.schema_mut(),
        }
    }

    pub fn set_schema(&mut self, schema: super::EdgeSchema) {
        match self {
            EdgeStore::TimeTravel(s) => s.set_schema(schema),
            EdgeStore::Simple(s) => s.set_schema(schema),
        }
    }

    pub fn set_stats_manager(&mut self, stats: std::sync::Arc<crate::core::stats::StatsManager>) {
        match self {
            EdgeStore::TimeTravel(s) => s.set_stats_manager(stats),
            EdgeStore::Simple(s) => s.set_stats_manager(stats),
        }
    }

    pub fn version_history_ref(&self) -> std::sync::Arc<std::sync::Mutex<crate::storage::schema::LabelVersionHistory>> {
        match self {
            EdgeStore::TimeTravel(s) => s.version_history_ref(),
            EdgeStore::Simple(s) => s.version_history_ref(),
        }
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
        match self {
            EdgeStore::TimeTravel(s) => s.insert_edge(src, dst, rank, property_values, ts),
            EdgeStore::Simple(s) => s.insert_edge(src, dst, rank, property_values, ts),
        }
    }

    pub fn delete_edge(&mut self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> StorageResult<bool> {
        match self {
            EdgeStore::TimeTravel(s) => s.delete_edge(src, dst, rank, ts),
            EdgeStore::Simple(s) => s.delete_edge(src, dst, rank, ts),
        }
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
        match self {
            EdgeStore::TimeTravel(s) => s.delete_edge_by_offset(src, dst, rank, oe_offset, ie_offset, ts),
            EdgeStore::Simple(s) => s.delete_edge_by_offset(src, dst, rank, oe_offset, ie_offset, ts),
        }
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
        match self {
            EdgeStore::TimeTravel(s) => s.revert_delete_edge_by_offset(src, dst, rank, oe_offset, ie_offset, ts),
            EdgeStore::Simple(s) => s.revert_delete_edge_by_offset(src, dst, rank, oe_offset, ie_offset, ts),
        }
    }

    pub fn get_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<super::EdgeRecord> {
        match self {
            EdgeStore::TimeTravel(s) => s.get_edge(src, dst, rank, ts),
            EdgeStore::Simple(s) => s.get_edge(src, dst, rank, ts),
        }
    }

    pub fn has_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        match self {
            EdgeStore::TimeTravel(s) => s.has_edge(src, dst, rank, ts),
            EdgeStore::Simple(s) => s.has_edge(src, dst, rank, ts),
        }
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<super::EdgeRecord> {
        match self {
            EdgeStore::TimeTravel(s) => s.out_edges(src, ts),
            EdgeStore::Simple(s) => s.out_edges(src, ts),
        }
    }

    pub fn in_edges(&self, dst: u32, ts: Timestamp) -> Vec<super::EdgeRecord> {
        match self {
            EdgeStore::TimeTravel(s) => s.in_edges(dst, ts),
            EdgeStore::Simple(s) => s.in_edges(dst, ts),
        }
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
        match self {
            EdgeStore::TimeTravel(s) => s.update_edge_property(src, dst, rank, prop_name, value, ts),
            EdgeStore::Simple(s) => s.update_edge_property(src, dst, rank, prop_name, value, ts),
        }
    }

    pub fn update_edge_property_by_offset(
        &mut self,
        params: UpdateEdgePropertyByOffsetParams,
    ) -> StorageResult<bool> {
        match self {
            EdgeStore::TimeTravel(s) => s.update_edge_property_by_offset(params),
            EdgeStore::Simple(s) => s.update_edge_property_by_offset(params),
        }
    }

    // ── Schema operations ──
    pub fn add_property(
        &mut self,
        name: String,
        data_type: crate::core::DataType,
        nullable: bool,
    ) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.add_property(name, data_type, nullable),
            EdgeStore::Simple(s) => s.add_property(name, data_type, nullable),
        }
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.remove_property(name),
            EdgeStore::Simple(s) => s.remove_property(name),
        }
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.rename_property(old_name, new_name),
            EdgeStore::Simple(s) => s.rename_property(old_name, new_name),
        }
    }

    pub fn rebuild_schema_change_from_redo(&mut self, details: crate::storage::schema::ChangeDetails) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.rebuild_schema_change_from_redo(details),
            EdgeStore::Simple(s) => s.rebuild_schema_change_from_redo(details),
        }
    }

    // ── Query ──
    pub fn scan(&self, ts: Timestamp) -> Vec<super::EdgeRecord> {
        match self {
            EdgeStore::TimeTravel(s) => s.scan(ts),
            EdgeStore::Simple(s) => s.scan(ts),
        }
    }

    pub fn scan_paginated(&self, ts: Timestamp, offset: usize, page_size: usize) -> (Vec<super::EdgeRecord>, bool) {
        match self {
            EdgeStore::TimeTravel(s) => s.scan_paginated(ts, offset, page_size),
            EdgeStore::Simple(s) => s.scan_paginated(ts, offset, page_size),
        }
    }

    pub fn scan_paginated_iter(&self, ts: Timestamp, offset: usize, page_size: usize) -> EdgeStoreScanIterator<'_> {
        match self {
            EdgeStore::TimeTravel(s) => EdgeStoreScanIterator::TimeTravel(s.scan_paginated_iter(ts, offset, page_size)),
            EdgeStore::Simple(s) => EdgeStoreScanIterator::Simple(s.scan_paginated_iter(ts, offset, page_size)),
        }
    }

    pub fn edge_count(&self) -> u64 {
        match self {
            EdgeStore::TimeTravel(s) => s.edge_count(),
            EdgeStore::Simple(s) => s.edge_count(),
        }
    }

    pub fn delta_edge_count(&self) -> u64 {
        match self {
            EdgeStore::TimeTravel(s) => s.delta_edge_count(),
            EdgeStore::Simple(s) => s.delta_edge_count(),
        }
    }

    // ── Maintenance ──
    pub fn freeze_csr_only(&mut self, ts: Timestamp) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.freeze_csr_only(ts),
            EdgeStore::Simple(s) => s.freeze_csr_only(ts),
        }
    }

    pub fn compact_and_freeze(&mut self, ts: Timestamp, config: &CompactConfig, mode: CompactionMode) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.compact_and_freeze(ts, config, mode),
            EdgeStore::Simple(s) => s.compact_and_freeze(ts, config, mode),
        }
    }

    pub fn compact_properties(&mut self, ts: Timestamp) {
        match self {
            EdgeStore::TimeTravel(s) => s.compact_properties(ts),
            EdgeStore::Simple(s) => s.compact_properties(ts),
        }
    }

    pub fn compact_csr_only(&mut self, ts: Timestamp, reserve_ratio: f32) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.compact_csr_only(ts, reserve_ratio),
            EdgeStore::Simple(s) => s.compact_csr_only(ts, reserve_ratio),
        }
    }

    pub fn maybe_compact_for_flush(&mut self, ts: Timestamp, threshold: f32) {
        match self {
            EdgeStore::TimeTravel(s) => s.maybe_compact_for_flush(ts, threshold),
            EdgeStore::Simple(s) => s.maybe_compact_for_flush(ts, threshold),
        }
    }

    pub fn merge_segments_lsm_tiered(&mut self, current_ts: Timestamp) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.merge_segments_lsm_tiered(current_ts),
            EdgeStore::Simple(s) => s.merge_segments_lsm_tiered(current_ts),
        }
    }

    pub fn merge_segments_adaptive(
        &mut self,
        current_ts: Timestamp,
        max_segment_age: Timestamp,
        deletion_threshold: f64,
        max_segment_size_bytes: usize,
    ) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.merge_segments_adaptive(current_ts, max_segment_age, deletion_threshold, max_segment_size_bytes),
            EdgeStore::Simple(s) => s.merge_segments_adaptive(current_ts, max_segment_age, deletion_threshold, max_segment_size_bytes),
        }
    }

    pub fn merge_segments_with_config(
        &mut self,
        time_threshold: Timestamp,
        size_threshold_bytes: usize,
    ) -> MergeMetricsResult {
        match self {
            EdgeStore::TimeTravel(s) => s.merge_segments_with_config(time_threshold, size_threshold_bytes),
            EdgeStore::Simple(s) => s.merge_segments_with_config(time_threshold, size_threshold_bytes),
        }
    }

    pub fn merge_segments_with_config_and_deletion_filter(
        &mut self,
        time_threshold: Timestamp,
        size_threshold_bytes: usize,
        min_active_snapshot_ts: Option<Timestamp>,
    ) -> MergeMetricsResult {
        match self {
            EdgeStore::TimeTravel(s) => s.merge_segments_with_config_and_deletion_filter(time_threshold, size_threshold_bytes, min_active_snapshot_ts),
            EdgeStore::Simple(s) => s.merge_segments_with_config_and_deletion_filter(time_threshold, size_threshold_bytes, min_active_snapshot_ts),
        }
    }

    pub fn merge_stats(&self) -> MergeStats {
        match self {
            EdgeStore::TimeTravel(s) => s.merge_stats(),
            EdgeStore::Simple(s) => s.merge_stats(),
        }
    }

    pub fn deletion_stats(&self) -> stats::DeletionStats {
        match self {
            EdgeStore::TimeTravel(s) => s.deletion_stats(),
            EdgeStore::Simple(s) => s.deletion_stats(),
        }
    }

    pub fn validate_segment_integrity(&self) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.validate_segment_integrity(),
            EdgeStore::Simple(s) => s.validate_segment_integrity(),
        }
    }

    pub fn segment_versions(&self) -> Vec<(usize, u32)> {
        match self {
            EdgeStore::TimeTravel(s) => s.segment_versions(),
            EdgeStore::Simple(s) => s.segment_versions(),
        }
    }

    pub fn update_segment_checksums(&mut self) {
        match self {
            EdgeStore::TimeTravel(s) => s.update_segment_checksums(),
            EdgeStore::Simple(s) => s.update_segment_checksums(),
        }
    }

    // ── Memory ──
    pub fn memory_size(&self) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.memory_size(),
            EdgeStore::Simple(s) => s.memory_size(),
        }
    }

    pub fn used_memory_size(&self) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.used_memory_size(),
            EdgeStore::Simple(s) => s.used_memory_size(),
        }
    }

    pub fn mutable_csr_memory_size(&self) -> usize {
        match self {
            EdgeStore::TimeTravel(s) => s.mutable_csr_memory_size(),
            EdgeStore::Simple(s) => s.mutable_csr_memory_size(),
        }
    }

    // ── Snapshots ──
    pub fn register_snapshot(&mut self, ts: Timestamp) {
        match self {
            EdgeStore::TimeTravel(s) => s.register_snapshot(ts),
            EdgeStore::Simple(s) => s.register_snapshot(ts),
        }
    }

    pub fn unregister_snapshot(&mut self, ts: Timestamp) {
        match self {
            EdgeStore::TimeTravel(s) => s.unregister_snapshot(ts),
            EdgeStore::Simple(s) => s.unregister_snapshot(ts),
        }
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<ExportedEdgeSnapshot> {
        match self {
            EdgeStore::TimeTravel(s) => s.export_snapshot(ts),
            EdgeStore::Simple(s) => s.export_snapshot(ts),
        }
    }

    pub fn snapshot_handle(&mut self, ts: Timestamp) -> EdgeSnapshotHandle<'_> {
        self.register_snapshot(ts);
        EdgeSnapshotHandle { table: self, ts }
    }

    // ── Persistence ──
    pub fn flush<P: AsRef<std::path::Path>>(
        &self,
        path: P,
        compression: crate::storage::compression::CompressionType,
    ) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.flush(path, compression),
            EdgeStore::Simple(s) => s.flush(path, compression),
        }
    }

    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        match self {
            EdgeStore::TimeTravel(s) => s.load(path),
            EdgeStore::Simple(s) => s.load(path),
        }
    }
}

// ── RAII snapshot handle (works with EdgeStore) ──
/// RAII handle for edge table snapshots.
///
/// Automatically unregisters the snapshot from MVCC tracking when dropped,
/// enabling proper tombstone garbage collection.
pub struct EdgeSnapshotHandle<'a> {
    table: &'a mut EdgeStore,
    ts: Timestamp,
}

impl<'a> EdgeSnapshotHandle<'a> {
    pub fn timestamp(&self) -> Timestamp {
        self.ts
    }

    pub fn export(&self) -> StorageResult<ExportedEdgeSnapshot> {
        self.table.export_snapshot(self.ts)
    }

    pub fn release(mut self) {
        self.ts = u32::MAX;
    }
}

impl<'a> Drop for EdgeSnapshotHandle<'a> {
    fn drop(&mut self) {
        if self.ts != u32::MAX {
            self.table.unregister_snapshot(self.ts);
        }
    }
}

// ── Unified scan iterator for EdgeStore ──
pub enum EdgeStoreScanIterator<'a> {
    TimeTravel(core::EdgeTableScanIterator<'a>),
    Simple(simple::SimpleScanIterator<'a>),
}

impl<'a> Iterator for EdgeStoreScanIterator<'a> {
    type Item = super::EdgeRecord;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            EdgeStoreScanIterator::TimeTravel(iter) => iter.next(),
            EdgeStoreScanIterator::Simple(iter) => iter.next(),
        }
    }
}

impl<'a> EdgeStoreScanIterator<'a> {
    pub fn has_more(&self) -> bool {
        match self {
            EdgeStoreScanIterator::TimeTravel(iter) => iter.has_more(),
            EdgeStoreScanIterator::Simple(iter) => iter.has_more(),
        }
    }
}

// ── TimeTravelEdgeStore methods (from impl EdgeTable alias) ──
impl EdgeTable {
    pub fn register_snapshot(&mut self, ts: Timestamp) {
        self.mvcc.register_active_snapshot(ts);
    }

    pub fn unregister_snapshot(&mut self, ts: Timestamp) {
        self.mvcc.unregister_active_snapshot(ts);
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<ExportedEdgeSnapshot> {
        use snapshot::SnapshotBuilder;
        let out_edges = self.collect_edges_for_snapshot_mvcc(&self.out_csr, &self.out_segments, ts)?;
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
                && segment.deletion_info.all_edges_deleted(segment.csr.edge_count())
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
        let out_reduced = merge::merge_lsm_tiered(&mut self.out_segments, current_ts);
        let in_reduced = merge::merge_lsm_tiered(&mut self.in_segments, current_ts);

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
        let out_reduced = merge::merge_adaptive(
            &mut self.out_segments, current_ts, max_segment_age, deletion_threshold, max_segment_size_bytes,
        );
        let in_reduced = merge::merge_adaptive(
            &mut self.in_segments, current_ts, max_segment_age, deletion_threshold, max_segment_size_bytes,
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

        let out_metrics = merge::merge_in_place(&mut self.out_segments, time_threshold, size_threshold_bytes);
        let in_metrics = merge::merge_in_place(&mut self.in_segments, time_threshold, size_threshold_bytes);

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
                merge::merge_selected_segments_with_deletion_filter(
                    &mut self.out_segments, out_indices, u32::MAX, min_active_snapshot_ts,
                );
            }
            if self.in_segments.len() > 1 {
                let in_indices: Vec<usize> = (0..self.in_segments.len()).collect();
                merge::merge_selected_segments_with_deletion_filter(
                    &mut self.in_segments, in_indices, u32::MAX, min_active_snapshot_ts,
                );
            }
        } else {
            let _ = merge::merge_in_place(&mut self.out_segments, time_threshold, size_threshold_bytes);
            let _ = merge::merge_in_place(&mut self.in_segments, time_threshold, size_threshold_bytes);
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
            &self.out_csr, &self.out_segments, &out_csr_path,
            crate::storage::persistence::section::EDGE_OUT_CSR,
        )?;
        crate::storage::compression::compress_file_inplace(&out_csr_path, compression)?;

        let in_csr_path = path.join("in_csr.bin");
        persistence::flush_csr(
            &self.in_csr, &self.in_segments, &in_csr_path,
            crate::storage::persistence::section::EDGE_IN_CSR,
        )?;
        crate::storage::compression::compress_file_inplace(&in_csr_path, compression)?;

        let props_path = path.join("properties.bin");
        persistence::flush_properties(&self.properties, &props_path)?;
        crate::storage::compression::compress_file_inplace(&props_path, compression)?;

        Ok(())
    }

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
                    crate::storage::persistence::section::EDGE_META, sid
                )));
            }
        }

        let mut version_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut version_bytes)?;
        let version = u32::from_le_bytes(version_bytes);
        if version != 2 {
            return Err(StorageError::deserialize_error(format!(
                "unsupported edge meta version: {}", version
            )));
        }

        let (label, src_label, dst_label, label_name, is_open, schema, next_edge_id, tombstones, min_snapshot_ts) =
            persistence::load_metadata(&mut meta_cursor)?;

        self.label = label;
        self.src_label = src_label;
        self.dst_label = dst_label;
        self.label_name = label_name;
        self.is_open = is_open;
        self.set_schema(schema);
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
            oe_strategy: super::super::EdgeStrategy::Multiple,
            ie_strategy: super::super::EdgeStrategy::Multiple,
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
