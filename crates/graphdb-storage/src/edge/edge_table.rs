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

pub mod calibrator;
pub mod compaction;
pub mod config;
pub mod core;
pub mod free_space;
pub mod freeze;
pub mod iterator;
pub mod merge;
pub mod mvcc;
pub mod page_state;
pub mod persistence;
pub mod remap;
pub mod residency;
pub mod segment;
pub mod segment_eviction;
pub mod snapshot;
pub mod stats;

// Re-export commonly used types
pub use core::UpdateEdgePropertyByOffsetParams;
pub use segment::CsrSegment;
pub use snapshot::ExportedEdgeSnapshot;
pub use stats::{MergeMetrics, MergeMetricsResult, MergeStats};

// Re-export from parent
pub use super::{CsrBase, CsrVariant, Nbr};

use crate::cold::{ColdPropertyIndex, ColdSnapshot};
use crate::edge::edge_table::core::EdgeTableConfig;
use crate::edge::edge_table::snapshot::max_edge_row;
use crate::persistence::write_header_to;
use graphdb_core::types::CompactConfig;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};
use std::fmt;
use std::path::Path;
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

    /// Run automatic maintenance on this table (tombstone GC, property
    /// compaction, delta freeze) based on configured thresholds.
    /// Returns the number of maintenance passes that actually ran.
    pub fn maybe_run_auto_maintenance(&mut self) -> usize {
        self.0.maybe_run_auto_maintenance()
    }

    /// Unified watermark variant. Shares the same safe cutoff across all
    /// table types in one GC pass so prefix reclaim cannot affect later types.
    pub fn maybe_run_auto_maintenance_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        self.0
            .maybe_run_auto_maintenance_with_watermarks(watermarks, margin)
    }

    /// GC column version chains using unified watermarks.
    pub fn gc_column_versions_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        self.0
            .properties
            .gc_versions_with_watermarks(watermarks, margin)
    }

    /// Get the current tombstone statistics for observability and GC decisions.
    pub fn tombstone_stats(&self) -> stats::TombstoneStats {
        self.0.mvcc.tombstone_stats()
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

    pub fn set_stats_manager(&mut self, stats: std::sync::Arc<graphdb_core::stats::StatsManager>) {
        self.0.set_stats_manager(stats)
    }

    pub fn version_history_ref(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<crate::schema::LabelVersionHistory>> {
        self.0.version_history_ref()
    }

    // ── CRUD ──
    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, graphdb_core::Value)],
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

    /// Raw out-edge neighbors of `src` without property decoding.
    pub fn merged_out_nbrs(&self, src: u32, ts: Timestamp) -> Vec<super::Nbr> {
        self.0.merged_out_nbrs(src, ts)
    }

    /// Raw in-edge neighbors of `dst` without property decoding.
    pub fn merged_in_nbrs(&self, dst: u32, ts: Timestamp) -> Vec<super::Nbr> {
        self.0.merged_in_nbrs(dst, ts)
    }

    pub fn update_edge_property(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        prop_name: &str,
        value: &graphdb_core::Value,
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
        data_type: graphdb_core::DataType,
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
        details: crate::schema::ChangeDetails,
    ) -> StorageResult<()> {
        self.0.rebuild_schema_change_from_redo(details)
    }

    // ── Query ──
    pub fn scan(&self, ts: Timestamp) -> Vec<super::EdgeRecord> {
        self.0.scan(ts)
    }

    /// Optimizer-facing statistics snapshot for one property column.
    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        self.0.column_stats_snapshot(column)
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

    pub fn compact_and_freeze(&mut self, ts: Timestamp, config: &CompactConfig) -> usize {
        self.0.compact_and_freeze(ts, config)
    }

    /// Set the operator retention floor for reclamation without active
    /// snapshots (`0` disables). See `MVCCManager::effective_retention_bound`.
    pub fn set_retention_floor(&mut self, floor: Timestamp) {
        self.0.mvcc.set_retention_floor(floor);
    }

    pub fn compact_properties(&mut self, ts: Timestamp) {
        self.0.compact_properties(ts)
    }

    /// Propagate vertex compaction internal-ID remaps (src and dst label
    /// spaces) into CSR rows, neighbors, segments, and derived indexes.
    pub fn remap_vertex_ids(
        &mut self,
        src_mapping: Option<&std::collections::HashMap<u32, u32>>,
        dst_mapping: Option<&std::collections::HashMap<u32, u32>>,
    ) -> StorageResult<()> {
        self.0.remap_vertex_ids(src_mapping, dst_mapping)
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

    pub fn calibrated_threshold(&self) -> super::edge_table::calibrator::CalibratedThreshold {
        self.0.calibrated_threshold()
    }

    pub fn overflow_index_stats(&self) -> Option<super::mutable_csr::OverflowIndexStats> {
        self.0.out_csr.overflow_index_stats()
    }

    pub fn rebuild_overflow_index(&mut self) {
        self.0.out_csr.rebuild_overflow_index();
        self.0.in_csr.rebuild_overflow_index();
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

    pub fn export_snapshot_file<P: AsRef<Path>>(
        &self,
        ts: Timestamp,
        path: P,
    ) -> StorageResult<ColdSnapshot> {
        self.0.export_snapshot_file(ts, path)
    }

    pub fn export_snapshot_file_with_retention<P: AsRef<Path>>(
        &self,
        ts: Timestamp,
        keep_recent: u64,
        path: P,
    ) -> StorageResult<ColdSnapshot> {
        self.0
            .export_snapshot_file_with_retention(ts, keep_recent, path)
    }

    /// Evict frozen edges from the hot store: every edge visible at `ts`
    /// except the `keep_recent` newest ones is MVCC-deleted at `ts`, so reads
    /// at or after `ts` fall through to the matching cold snapshot.
    pub fn freeze_edges_before(&mut self, ts: Timestamp, keep_recent: u64) -> StorageResult<u64> {
        self.0.freeze_edges_before(ts, keep_recent)
    }

    // ── Edge Property Index ──
    pub fn enable_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        self.0.enable_property_index(pool_capacity)
    }

    pub fn has_property_index(&self) -> bool {
        self.0.has_property_index()
    }

    pub fn disable_property_index(&mut self) {
        self.0.disable_property_index();
    }

    pub fn lookup_edges_by_property_range(
        &self,
        prop_name: &str,
        value_lower: &[u8],
        value_upper: &[u8],
    ) -> Vec<(u32, u32, i64)> {
        self.0
            .lookup_edges_by_property_range(prop_name, value_lower, value_upper)
    }

    // ── Persistence ──
    pub fn flush<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
        compression: crate::compression::CompressionType,
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
        self.properties
            .set_retention_horizon(self.mvcc.min_active_snapshot_ts);
    }

    pub fn unregister_snapshot(&mut self, ts: Timestamp) {
        self.mvcc.unregister_active_snapshot(ts);
        self.properties
            .set_retention_horizon(self.mvcc.min_active_snapshot_ts);
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<ExportedEdgeSnapshot> {
        self.export_snapshot_with_retention(ts, 0)
    }

    /// Export a snapshot at `ts`, excluding the `keep_recent` newest edges.
    ///
    /// Edges are ranked newest-first by (create_ts, edge_id); the same
    /// ordering is used by [`Self::freeze_edges_before`], so the exported
    /// set and the hot-evicted set stay identical.
    pub fn export_snapshot_with_retention(
        &self,
        ts: Timestamp,
        keep_recent: u64,
    ) -> StorageResult<ExportedEdgeSnapshot> {
        use snapshot::SnapshotBuilder;
        let mut out_edges =
            self.collect_edges_for_snapshot_mvcc(&self.out_csr, &self.out_segments, ts)?;
        let mut in_edges =
            self.collect_edges_for_snapshot_mvcc(&self.in_csr, &self.in_segments, ts)?;

        if keep_recent > 0 {
            let newest_first = |a: &(u32, Nbr, Timestamp), b: &(u32, Nbr, Timestamp)| {
                b.2.cmp(&a.2).then_with(|| b.1.edge_id.cmp(&a.1.edge_id))
            };
            out_edges.sort_by(newest_first);
            in_edges.sort_by(newest_first);
            let cut = out_edges.len().saturating_sub(keep_recent as usize);
            out_edges.truncate(cut);
            in_edges.truncate(cut);
        }

        let out_csr = {
            let cap = max_edge_row(&out_edges, self.out_csr.vertex_capacity());
            SnapshotBuilder::build_csr(out_edges.clone(), cap)?
        };
        let in_csr = {
            let cap = max_edge_row(&in_edges, self.in_csr.vertex_capacity());
            SnapshotBuilder::build_csr(in_edges.clone(), cap)?
        };

        // Build snapshot CsrWithProperties from existing CsrWithProperties (convert at ts)
        let prop_schemas: Vec<crate::edge::property_schema::PropertySchema> = self
            .schema
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| {
                crate::edge::property_schema::PropertySchema::new(
                    p.name.clone(),
                    i as i32,
                    p.data_type.clone(),
                )
                .nullable(p.nullable)
            })
            .collect();
        let mut snap_props = crate::edge::CsrWithProperties::new(
            out_csr.vertex_capacity().max(in_csr.vertex_capacity()),
            prop_schemas,
        );
        snap_props.set_version_chain_cap(self.config.version_chain_cap);
        let mut seen_ids = std::collections::HashSet::new();
        for (_, nbr, _) in out_edges.iter().chain(in_edges.iter()) {
            if !seen_ids.insert(nbr.edge_id) {
                continue;
            }
            let props_opt = self.properties.get_by_edge_id(nbr.edge_id, ts);
            if let Some(props) = props_opt {
                let values: Vec<(String, Value)> = props
                    .into_iter()
                    .filter_map(|(k, v)| v.map(|val| (k, val)))
                    .collect();
                if !values.is_empty() {
                    let _ = snap_props.insert_for_edge(nbr.edge_id, &values, ts);
                }
            }
        }

        Ok(ExportedEdgeSnapshot {
            snapshot_ts: ts,
            label: self.label,
            out_csr,
            in_csr,
            properties: snap_props,
            schema: self.schema.clone(),
        })
    }

    /// Evict every edge visible at `ts` except the `keep_recent` newest ones.
    ///
    /// Deletions happen at `ts` through the regular MVCC path, so reads
    /// before `ts` still see the edges while reads at or after `ts` are
    /// served by the matching cold snapshot. Returns the number of edges
    /// evicted.
    pub fn freeze_edges_before(&mut self, ts: Timestamp, keep_recent: u64) -> StorageResult<u64> {
        let mut edges =
            self.collect_edges_for_snapshot_mvcc(&self.out_csr, &self.out_segments, ts)?;
        edges.sort_by_key(|(_, nbr, create_ts)| std::cmp::Reverse((*create_ts, nbr.edge_id)));
        edges.truncate(edges.len().saturating_sub(keep_recent as usize));

        let mut evicted = 0u64;
        for (src, nbr, _) in edges {
            let dst_vid = nbr.to_vertex_id();
            let rank = nbr.rank;
            let dst = dst_vid.as_int64().unwrap_or(0) as u32;
            if self.delete_edge(src, dst, rank, ts)? {
                evicted += 1;
            }
        }
        Ok(evicted)
    }

    pub fn export_snapshot_file<P: AsRef<Path>>(
        &self,
        ts: Timestamp,
        path: P,
    ) -> StorageResult<ColdSnapshot> {
        self.export_snapshot_file_with_retention(ts, 0, path)
    }

    pub fn export_snapshot_file_with_retention<P: AsRef<Path>>(
        &self,
        ts: Timestamp,
        keep_recent: u64,
        path: P,
    ) -> StorageResult<ColdSnapshot> {
        let exported = self.export_snapshot_with_retention(ts, keep_recent)?;
        let index = self.property_index.as_ref().and_then(|index| {
            let names = index.indexed_property_names();
            if names.is_empty() {
                None
            } else {
                Some(ColdPropertyIndex::build(&exported, &names))
            }
        });
        ColdSnapshot::create_with_index(&exported, index, path)
    }

    fn collect_edges_for_snapshot_mvcc(
        &self,
        delta: &CsrVariant,
        segments: &[CsrSegment],
        ts: Timestamp,
    ) -> StorageResult<Vec<(u32, Nbr, Timestamp)>> {
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

        let delta_edges: Vec<(u32, Nbr, Timestamp)> = delta
            .iter(ts)
            .map(|(src, nbr)| {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                let create_ts = nbr.create_ts;
                (src_u32, nbr, create_ts)
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
            self.update_calibrator_from_segments();
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
            self.update_calibrator_from_segments();
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
            self.update_calibrator_from_segments();
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

        let region_n = self.config.region_vertex_count;
        let (out_metrics, in_metrics) = if let Some(min_ts) = min_active_snapshot_ts {
            // Physical merge respects the time/size thresholds and drops only
            // edges no active snapshot can observe.
            let mvcc = &self.mvcc;
            let (out_metrics, in_metrics) = if region_n > 0 {
                let out_metrics = merge::merge_in_place_region_aware_with_free_space(
                    &mut self.out_segments,
                    time_threshold,
                    size_threshold_bytes,
                    min_ts,
                    &mut self.out_free_space,
                    &|edge_id| mvcc.delete_ts_of(edge_id),
                    region_n,
                );
                let in_metrics = merge::merge_in_place_region_aware_with_free_space(
                    &mut self.in_segments,
                    time_threshold,
                    size_threshold_bytes,
                    min_ts,
                    &mut self.in_free_space,
                    &|edge_id| mvcc.delete_ts_of(edge_id),
                    region_n,
                );
                (out_metrics, in_metrics)
            } else {
                let out_metrics = merge::merge_in_place_physical_with_free_space(
                    &mut self.out_segments,
                    time_threshold,
                    size_threshold_bytes,
                    min_ts,
                    &mut self.out_free_space,
                    &|edge_id| mvcc.delete_ts_of(edge_id),
                );
                let in_metrics = merge::merge_in_place_physical_with_free_space(
                    &mut self.in_segments,
                    time_threshold,
                    size_threshold_bytes,
                    min_ts,
                    &mut self.in_free_space,
                    &|edge_id| mvcc.delete_ts_of(edge_id),
                );
                (out_metrics, in_metrics)
            };
            (out_metrics, in_metrics)
        } else {
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
            (out_metrics, in_metrics)
        };

        let segments_after = self.out_segments.len() + self.in_segments.len();
        let duration_ms = start.elapsed().as_millis() as u64;

        if segments_before != segments_after {
            self.rebuild_segment_indices();
            self.rebuild_sparse_vertex_indices();
            self.rebuild_current_snapshot();
            self.update_calibrator_from_segments();
        }

        MergeMetricsResult {
            metrics: MergeMetrics {
                segments_before,
                segments_after,
                edges_merged: out_metrics.edges_processed + in_metrics.edges_processed,
                duration_ms,
            },
            segments_reduced: segments_before.saturating_sub(segments_after),
        }
    }

    pub fn flush<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
        compression: crate::compression::CompressionType,
    ) -> StorageResult<()> {
        use std::fs;
        let path = path.as_ref();
        fs::create_dir_all(path)?;
        crate::compression::cleanup_shadow_files(path)?;

        let crate::compression::CompressionType::Zstd { level } = compression;
        let page_size = crate::compression::DEFAULT_PAGE_SIZE;

        let mut meta_payload = Vec::new();
        write_header_to(&mut meta_payload, crate::persistence::section::EDGE_META).map_err(
            |e| StorageError::io_error(format!("Failed to write edge meta header: {}", e)),
        )?;

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
            &self.mvcc.edge_timestamps,
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
            crate::persistence::section::EDGE_OUT_CSR,
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
            crate::persistence::section::EDGE_IN_CSR,
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
        persistence::serialize_csr_properties(&self.properties, &mut props_payload)?;
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
        // Development builds keep a single on-disk layout; version numbers
        // only start to accumulate after the first release.
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
        self.mvcc.tombstones = meta.tombstones;
        self.mvcc.min_active_snapshot_ts = meta.min_snapshot_ts;
        self.mvcc.edge_timestamps = meta.edge_timestamps;
        self.properties
            .set_retention_horizon(self.mvcc.min_active_snapshot_ts);
        self.out_free_space.clear();
        self.in_free_space.clear();

        let out_csr_path = path.join("out_csr.bin");
        persistence::load_csr(&out_csr_path, &mut self.out_csr, &mut self.out_segments)?;

        let in_csr_path = path.join("in_csr.bin");
        persistence::load_csr(&in_csr_path, &mut self.in_csr, &mut self.in_segments)?;

        // Rebuild region metadata for segments lacking it (old files without region block)
        let region_n = self.config.region_vertex_count;
        if region_n > 0 {
            for seg in &mut self.out_segments {
                if seg.regions.is_empty() {
                    seg.rebuild_regions(region_n, &|eid| self.mvcc.delete_ts_of(eid));
                }
            }
            for seg in &mut self.in_segments {
                if seg.regions.is_empty() {
                    seg.rebuild_regions(region_n, &|eid| self.mvcc.delete_ts_of(eid));
                }
            }
        }
        // Rebuild calibrator and overflow index after load
        self.update_calibrator_from_segments();
        self.out_csr.rebuild_overflow_index();
        self.in_csr.rebuild_overflow_index();

        // Rebuild inline create_ts from persisted edge_timestamps
        use crate::edge::csr_trait::MutableCsrTrait;
        let create_ts_iter = self
            .mvcc
            .edge_timestamps
            .iter()
            .map(|(&eid, ts)| (eid, ts.create_ts));
        let create_ts_vec: Vec<_> = create_ts_iter.collect();
        self.out_csr
            .rebuild_create_ts(create_ts_vec.iter().copied());
        self.in_csr.rebuild_create_ts(create_ts_vec.into_iter());

        let props_path = path.join("properties.bin");
        self.properties = {
            let p = persistence::load_csr_properties(&props_path)?;
            let mut new_props = p;
            // Rebuild columns to match current schema if needed
            let current_schema_names: std::collections::HashSet<_> =
                self.schema.properties.iter().map(|p| &p.name).collect();
            let existing_names: std::collections::HashSet<_> = new_props
                .property_schema()
                .iter()
                .map(|s| &s.name)
                .collect();
            if current_schema_names != existing_names {
                // Schema mismatch: rebuild from schema
                let prop_schemas: Vec<crate::edge::property_schema::PropertySchema> = self
                    .schema
                    .properties
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        crate::edge::property_schema::PropertySchema::new(
                            p.name.clone(),
                            i as i32,
                            p.data_type.clone(),
                        )
                        .nullable(p.nullable)
                    })
                    .collect();
                let mut rebuilt =
                    crate::edge::CsrWithProperties::new(new_props.vertex_capacity(), prop_schemas);
                rebuilt.set_version_chain_cap(self.config.version_chain_cap);
                new_props = rebuilt;
            }
            new_props
        };

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
    use crate::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use crate::edge::CsrBase;
    use crate::edge::EdgeSchema;
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;
    use graphdb_core::Value;

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
    fn test_sparse_high_ids_keep_csr_rows_proportional() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        // Sparse writes with a few high internal ids. With dense internal ids
        // this must NOT amplify CSR rows (old behavior: ~16x via per-row
        // primary pre-allocation plus power-of-two growth).
        table.insert_edge(0, 100_000, 0, &[], 100).unwrap();
        table.insert_edge(50_000, 100_001, 0, &[], 100).unwrap();
        table.insert_edge(100_002, 0, 0, &[], 100).unwrap();

        let max_id = 100_002usize;
        let out_rows = table.out_csr.vertex_capacity();
        let in_rows = table.in_csr.vertex_capacity();

        // Proportional 1.25x stepping: rows ~= vertices + 25% tail
        assert!(out_rows <= ((max_id + 1) as f64 * 1.25).ceil() as usize);
        assert!(in_rows <= ((max_id + 1) as f64 * 1.25).ceil() as usize);
        assert!(out_rows > max_id);
        assert!(in_rows > max_id);

        // Zero-degree rows must hold no data slots in the CSR: wasted
        // capacity stays tiny (was ~16x rows before lazy allocation)
        assert!(table.out_csr.wasted_bytes_estimate() < 64 * 64);
        assert!(table.in_csr.wasted_bytes_estimate() < 64 * 64);
    }

    #[test]
    fn test_row_space_footprint_patterns() {
        let schema = create_test_schema();
        let ts = 100u64;
        let n = 100_000u32;

        // Pattern 1: dense sequential ids.
        let mut dense =
            TimeTravelEdgeStore::with_config(schema.clone(), EdgeTableConfig::default()).unwrap();
        for src in 0..n {
            dense.insert_edge(src, (src + 1) % n, 0, &[], ts).unwrap();
        }

        // Pattern 2: dense core + a few edges at very high ids (the internal
        // id shape that sparse external ids produced before densification).
        // Row *arrays* track the highest id by design (1.25x tail); the lazy
        // allocation win is that edge *data* and memory stay proportional to
        // the edge count, not the id range.
        let mut sparse =
            TimeTravelEdgeStore::with_config(schema.clone(), EdgeTableConfig::default()).unwrap();
        for src in 0..n {
            sparse.insert_edge(src, (src + 1) % n, 0, &[], ts).unwrap();
        }
        for high in [1_000_000u32, 2_000_000, 4_000_000] {
            sparse.insert_edge(high, 0, 0, &[], ts).unwrap();
        }

        // Pattern 3: delete half the edges and reinsert them.
        let mut reinsert =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();
        for src in 0..n {
            reinsert
                .insert_edge(src, (src + 1) % n, 0, &[], ts)
                .unwrap();
        }
        for src in 0..n / 2 {
            let _ = reinsert.delete_edge(src, (src + 1) % n, 0, ts + 1);
        }
        for src in 0..n / 2 {
            reinsert
                .insert_edge(src, (src + 1) % n, 1, &[], ts + 2)
                .unwrap();
        }

        let footprint = |table: &TimeTravelEdgeStore| {
            let (out_rows, in_rows) = (
                table.out_csr.vertex_capacity(),
                table.in_csr.vertex_capacity(),
            );
            let mem = table.mutable_csr_memory_size();
            println!(
                "row footprint: out_rows={out_rows}, in_rows={in_rows}, mutable_memory={mem} B, edges={}",
                table.edge_count()
            );
            (out_rows, in_rows, mem)
        };

        println!("dense_sequential:"); // prefix for pattern identification
        let (dense_out, dense_in, dense_mem) = footprint(&dense);
        println!("sparse_high_id:");
        let (sparse_out, sparse_in, sparse_mem) = footprint(&sparse);
        println!("delete_reinsert:");
        let (reinsert_out, reinsert_in, _) = footprint(&reinsert);

        let tail = |rows: usize| ((rows as f64) * 1.25).ceil() as usize;

        // Rows track the highest edge-bearing id with at most the 1.25x
        // growth tail.
        assert!(dense_out <= tail(n as usize));
        assert!(dense_in <= tail(n as usize));
        assert!(sparse_out <= tail(4_000_002));
        assert!(sparse_in <= tail(n as usize));

        // Memory follows the edge count, not the id range: +3 edges in the
        // sparse pattern must not inflate memory (pre-lazy-allocation this
        // allocated per-row edge blocks across the whole 4M-id range).
        assert!(
            sparse_mem <= dense_mem + 64 * 64,
            "sparse high ids must not inflate mutable memory: dense={dense_mem}, sparse={sparse_mem}"
        );

        // Tombstoned deletions do not expand rows; reinserts reuse slots.
        assert!(reinsert_out <= tail(n as usize));
        assert!(reinsert_in <= tail(n as usize));
    }

    #[test]
    fn test_row_capacity_assertion_on_compaction() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        // Create sparse IDs with deletions and reinserts
        table.insert_edge(0, 500_000, 0, &[], 100).unwrap();
        table.insert_edge(50_000, 100_001, 0, &[], 100).unwrap();
        table.insert_edge(100_002, 0, 0, &[], 100).unwrap();

        // Delete and reinsert
        table.delete_edge(0, 50_000, 0, 150).unwrap();
        table.insert_edge(0, 50_001, 1, &[], 160).unwrap();

        // Compact mutable CSR (layer 1 deletion)
        let _removed = table.compact_csr_only(200, 0.25);
        // The deleted edge may be in out_csr (deletion at timestamp 150)
        // The compaction at timestamp 200 may or may not remove it depending on visibility
        // Assert that row capacity is within bounds regardless of whether compaction removed edges

        // Row capacity should respect 1.25x growth factor.
        // out CSR tracks max src (100_002), in CSR tracks max dst (500_001 after reinsert).
        let max_src = 100_002usize;
        let max_dst = 500_001usize;
        let out_rows = table.out_csr.vertex_capacity();
        let in_rows = table.in_csr.vertex_capacity();
        let tail = |rows: usize| ((rows as f64) * 1.25).ceil() as usize;

        assert!(
            out_rows <= tail(max_src + 1),
            "out rows {} exceeds 1.25x tail of {}",
            out_rows,
            max_src + 1
        );
        assert!(
            in_rows <= tail(max_dst + 1),
            "in rows {} exceeds 1.25x tail of {}",
            in_rows,
            max_dst + 1
        );

        // Lazy allocation: wasted memory should stay tiny
        assert!(
            table.out_csr.wasted_bytes_estimate() < 64 * 64,
            "out CSR wasted memory {} exceeds lazy allocation tolerance",
            table.out_csr.wasted_bytes_estimate()
        );
        assert!(
            table.in_csr.wasted_bytes_estimate() < 64 * 64,
            "in CSR wasted memory {} exceeds lazy allocation tolerance",
            table.in_csr.wasted_bytes_estimate()
        );
    }

    #[test]
    fn test_row_capacity_assertion_on_freeze() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        // Freeze should truncate segments strictly to edge-bearing rows + 1
        table.insert_edge(0, 500_000, 0, &[], 100).unwrap();
        table.insert_edge(50_000, 100_001, 0, &[], 100).unwrap();
        table.insert_edge(100_002, 0, 0, &[], 100).unwrap();

        let frozen = table.freeze_csr_only(150);
        assert!(frozen > 0);

        // Frozen segment rows track only their own direction's vertex space
        let out_segment_capacity = table.out_segments[0].csr.read().vertex_capacity();
        let in_segment_capacity = table.in_segments[0].csr.read().vertex_capacity();

        // out segment: src IDs {0, 50_000, 100_002} -> max row 100_002 → capacity 100_003
        // in segment: dst IDs {500_000, 100_001, 0} -> max row 500_000 → capacity 500_001
        assert_eq!(out_segment_capacity, 100_003);
        assert_eq!(in_segment_capacity, 500_001);

        // After freeze, the mutable CSR is empty but retains its grown row capacity
        // (the frozen segment is what enforces strict truncation)
        let out_mutable_capacity = table.out_csr.vertex_capacity();
        let in_mutable_capacity = table.in_csr.vertex_capacity();
        let out_tail = ((100_003usize) as f64 * 1.25).ceil() as usize;
        let in_tail = ((500_001usize) as f64 * 1.25).ceil() as usize;
        assert!(
            out_mutable_capacity <= out_tail,
            "out mutable capacity {} exceeds 1.25x tail of 100_003",
            out_mutable_capacity
        );
        assert!(
            in_mutable_capacity <= in_tail,
            "in mutable capacity {} exceeds 1.25x tail of 500_001",
            in_mutable_capacity
        );
    }

    #[test]
    fn test_freeze_and_remap_keep_rows_truncated() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        // Sparse high ids, including a neighbor id (500_000) far above the
        // out-space row max (100_002): neighbor ids must not inflate rows.
        table.insert_edge(0, 500_000, 0, &[], 100).unwrap();
        table.insert_edge(50_000, 100_001, 0, &[], 100).unwrap();
        table.insert_edge(100_002, 0, 0, &[], 100).unwrap();
        table.freeze_csr_only(150);

        // Frozen segments truncate to the highest edge-bearing row + 1 in
        // their own direction's vertex space (out: src ids, in: dst ids).
        assert_eq!(table.out_segments[0].csr.read().vertex_capacity(), 100_003);
        assert_eq!(table.in_segments[0].csr.read().vertex_capacity(), 500_001);

        // Vertex compaction remaps live ids {0, 50_000, 100_000, 100_001,
        // 100_002, 500_000} to dense {0..=5}; mutable CSR and segment rows
        // follow and shrink.
        let mapping: std::collections::HashMap<u32, u32> = [
            (50_000, 1),
            (100_000, 2),
            (100_001, 3),
            (100_002, 4),
            (500_000, 5),
        ]
        .into_iter()
        .collect();
        table
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();

        // Mutable deltas are empty after freeze; the remap truncates them to
        // the single-row floor instead of keeping the pre-freeze capacity.
        // Segment rows follow the remap and shrink.
        assert_eq!(table.out_csr.vertex_capacity(), 1);
        assert_eq!(table.in_csr.vertex_capacity(), 1);
        assert_eq!(table.out_segments[0].csr.read().vertex_capacity(), 5);
        assert_eq!(table.in_segments[0].csr.read().vertex_capacity(), 6);

        // Queries resolve through the remapped rows/neighbors.
        assert!(table.get_edge(0, 5, 0, 200).is_some());
        assert!(table.get_edge(1, 3, 0, 200).is_some());
        assert!(table.get_edge(4, 0, 0, 200).is_some());
        assert_eq!(table.in_edges(5, 200).len(), 1);
        assert_eq!(table.in_edges(3, 200).len(), 1);
        assert_eq!(table.in_edges(0, 200).len(), 1);
    }

    #[test]
    fn test_freeze_csr_preserves_reads() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

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
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.freeze_csr_only(150);

        assert!(table.delete_edge(0, 1, 0, 200).unwrap());
        assert!(table.has_edge(0, 1, 0, 150));
        assert!(!table.has_edge(0, 1, 0, 250));
        assert_eq!(table.scan(250).len(), 0);
    }
}
