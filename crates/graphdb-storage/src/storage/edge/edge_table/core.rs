//! Core EdgeTable operations: CRUD, properties, queries, and compaction.
//!
//! Provides fundamental edge table functionality including insertion, deletion,
//! querying, property management, and basic maintenance operations.

use super::super::{Csr, CsrBase, CsrVariant, EdgeRecord, EdgeSchema, MutableCsrTrait, Nbr};
use super::free_space::SegmentFreeList;
use super::mvcc::MVCCManager;
use super::residency::GLOBAL_ACCESS_CLOCK;
use super::segment::{CsrSegment, SegmentVersion};
use crate::core::types::{CompactConfig, EdgeId, LabelId, Timestamp, VertexId};
use crate::core::{DataType, StorageError, StorageResult, Value};
use crate::storage::edge::PropertyTable;
use crate::storage::index::edge_index_manager::EdgePropertyIndex;
use crate::storage::schema::{
    ChangeDetails, LabelVersionHistory, PropertyChange, SchemaObjectType,
};
use crate::storage::types::{PropertyId, StoragePropertyDef};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct EdgeTableConfig {
    pub initial_vertex_capacity: usize,
    pub initial_edge_capacity: usize,
    /// Fixed number of edges allocated per high-degree overflow chunk.
    pub overflow_chunk_edges: usize,
    pub max_segments_per_direction: usize,
    /// Write backpressure: max size of mutable CSR (in bytes) before triggering freeze.
    /// Set to 0 to disable. Typical value: 100MB (100 * 1024 * 1024).
    pub max_mutable_csr_bytes: usize,

    /// Segment merge threshold: trigger auto-merge when segment count reaches this value.
    /// Default: 50 segments per direction before merging oldest segments.
    /// Set to 0 to disable auto-merge.
    pub segment_merge_threshold: usize,

    /// Merge behavior: how many segments to keep after merging.
    /// When merging is triggered and segment count exceeds threshold,
    /// keep only the N newest segments (others are merged).
    /// Default: 5 (keeps 5 newest, merges the rest).
    pub merge_keep_newest: usize,

    /// Automatic maintenance: run freeze / GC / property compaction on the
    /// write path when the configured thresholds are exceeded.
    pub auto_maintenance: AutoMaintenanceConfig,

    /// Region-level recycling: vertex count per region (0 = disabled).
    pub region_vertex_count: usize,

    /// Upper bound on the per-row before-image version chain length in the
    /// property table. `0` disables the bound (unbounded history).
    pub version_chain_cap: usize,
}

/// Thresholds that trigger automatic maintenance on the write path.
#[derive(Debug, Clone, Copy)]
pub struct AutoMaintenanceConfig {
    /// Run GC when the total tombstone count exceeds this value.
    /// Set to 0 to disable tombstone GC.
    pub tombstone_gc_threshold: usize,
    /// Run property compaction when deleted-but-not-reclaimed property rows
    /// exceed this ratio of total rows. Set to 0.0 to disable.
    pub property_compact_ratio: f32,
    /// Freeze the mutable CSR when its estimated memory exceeds this value.
    /// Set to 0 to disable (falls back to global `max_mutable_csr_bytes`).
    pub max_delta_memory_bytes: usize,
    /// Minimum serial number between automatic GC runs. Each time GC runs
    /// the serial is incremented; subsequent write-path calls skip GC until
    /// the counter reaches this value again. Set to 0 to disable cooldown.
    pub gc_min_serial: u64,
    /// Run a PhysicalDeletion segment merge when the deleted edge ratio in
    /// frozen segments exceeds this value (0.0 to 1.0). Set to 0.0 to disable.
    /// Edges are only physically dropped when an active snapshot bounds the
    /// retention horizon; without snapshots the merge is a no-op for reclamation.
    pub deletion_compact_ratio: f64,
}

impl Default for AutoMaintenanceConfig {
    fn default() -> Self {
        Self {
            tombstone_gc_threshold: 200_000,
            property_compact_ratio: 0.15,
            max_delta_memory_bytes: 150 * 1024 * 1024,
            gc_min_serial: 500,
            deletion_compact_ratio: 0.5,
        }
    }
}

impl Default for EdgeTableConfig {
    fn default() -> Self {
        Self {
            initial_vertex_capacity: 4096,
            initial_edge_capacity: 4096,
            overflow_chunk_edges: 4096,
            max_segments_per_direction: 100,
            // Default: 100MB per direction
            max_mutable_csr_bytes: 100 * 1024 * 1024,
            // Auto-merge when segment count reaches 50 per direction
            segment_merge_threshold: 50,
            // Keep only 5 newest segments, merge the rest (oldest 45 become 1)
            merge_keep_newest: 5,
            auto_maintenance: AutoMaintenanceConfig::default(),
            region_vertex_count: super::segment::DEFAULT_REGION_VERTEX_COUNT,
            version_chain_cap: crate::storage::edge::property_table::DEFAULT_VERSION_CHAIN_CAP,
        }
    }
}

/// Parameters for update_edge_property_by_offset operation
pub struct UpdateEdgePropertyByOffsetParams {
    pub src: u32,
    pub dst: u32,
    pub rank: i64,
    pub prop_id: u16,
    pub value: Value,
    pub ts: Timestamp,
}

/// TimeTravel edge store: multi-segment CSR with freeze/merge/MVCC (full history).
pub struct TimeTravelEdgeStore {
    pub label: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrVariant,
    pub in_csr: CsrVariant,
    pub out_segments: Vec<CsrSegment>,
    pub in_segments: Vec<CsrSegment>,
    /// Reusable CSR allocations retired from out-direction segments.
    pub out_free_space: SegmentFreeList,
    /// Reusable CSR allocations retired from in-direction segments.
    pub in_free_space: SegmentFreeList,
    /// Segment index for fast time-based lookup: (create_ts_min, segment_idx in out_segments)
    /// Sorted by create_ts_min, enables binary search to skip irrelevant segments
    pub out_segment_index: Vec<(Timestamp, usize)>,
    /// Segment index for in_segments: (create_ts_min, segment_idx in in_segments)
    pub in_segment_index: Vec<(Timestamp, usize)>,
    pub mvcc: MVCCManager,
    pub properties: PropertyTable,
    pub is_open: bool,
    pub next_edge_id: EdgeId,
    pub config: EdgeTableConfig,
    pub stats_manager: Option<std::sync::Arc<crate::core::stats::StatsManager>>,
    /// Version history tracking for schema changes
    pub version_history: Arc<Mutex<LabelVersionHistory>>,
    /// Cache for property name → schema index mapping to avoid O(n) linear lookups.
    /// Invalidated whenever schema changes.
    pub property_index_cache: HashMap<String, usize>,

    /// Sparse vertex index for out-direction segments.
    /// Maps source vertex ID → list of segment indices that contain edges for that vertex.
    /// Enables skipping segments that don't contain the queried vertex during traversal.
    pub sparse_vertex_index_out: HashMap<u32, Vec<usize>>,
    /// Sparse vertex index for in-direction segments.
    pub sparse_vertex_index_in: HashMap<u32, Vec<usize>>,

    /// Pre-merged CSR of all out-direction segments for ts=MAX queries.
    /// Built lazily when a current-time query arrives and invalidated on freeze/merge.
    pub current_snapshot_out: Option<Csr>,
    /// Pre-merged CSR of all in-direction segments for ts=MAX queries.
    pub current_snapshot_in: Option<Csr>,
    /// Whether the current snapshots need to be rebuilt.
    pub snapshot_dirty: bool,

    /// Edge property index for efficient property-based filtering.
    /// When set, insert/delete operations automatically maintain the index.
    pub property_index: Option<EdgePropertyIndex>,

    /// Serial counter for automatic maintenance: incremented on every
    /// maintenance run so tombstone GC can be rate-limited.
    pub maintenance_serial: u64,
    /// Snapshot timestamp used by the last automatic GC run. Used to avoid
    /// re-running GC when `min_active_snapshot_ts` has not advanced.
    pub last_gc_min_snapshot_ts: Timestamp,
}

impl TimeTravelEdgeStore {
    pub fn with_config(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        schema.validate()?;

        if config.overflow_chunk_edges == 0 {
            return Err(StorageError::invalid_operation(
                "overflow_chunk_edges must be greater than zero",
            ));
        }

        let out_csr = CsrVariant::from_strategy_with_overflow(
            schema.oe_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
            config.overflow_chunk_edges,
        )?;
        let in_csr = CsrVariant::from_strategy_with_overflow(
            schema.ie_strategy,
            config.initial_vertex_capacity,
            config.initial_edge_capacity,
            config.overflow_chunk_edges,
        )?;

        let mut properties = PropertyTable::with_capacity(config.initial_edge_capacity);
        properties.set_version_chain_cap(config.version_chain_cap);
        for prop in &schema.properties {
            properties.add_property(prop.name.clone(), prop.data_type.clone(), prop.nullable)?;
        }

        let label_id = schema.label_id;
        let label_name = schema.label_name.clone();

        let version_history = Arc::new(Mutex::new(LabelVersionHistory::new(
            label_id,
            label_name.clone(),
            SchemaObjectType::Edge,
        )));

        let mut property_index_cache = HashMap::new();
        for (idx, prop) in schema.properties.iter().enumerate() {
            property_index_cache.insert(prop.name.clone(), idx);
        }

        Ok(Self {
            label: label_id,
            label_name,
            src_label: schema.src_label,
            dst_label: schema.dst_label,
            schema,
            out_csr,
            in_csr,
            out_segments: Vec::new(),
            in_segments: Vec::new(),
            out_free_space: SegmentFreeList::new(),
            in_free_space: SegmentFreeList::new(),
            out_segment_index: Vec::new(),
            in_segment_index: Vec::new(),
            mvcc: MVCCManager::new(),
            properties,
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            version_history,
            property_index_cache,
            sparse_vertex_index_out: HashMap::new(),
            sparse_vertex_index_in: HashMap::new(),
            current_snapshot_out: None,
            current_snapshot_in: None,
            snapshot_dirty: true,
            property_index: None,
            maintenance_serial: 0,
            last_gc_min_snapshot_ts: 0,
        })
    }

    pub(crate) fn edge_endpoint_key(endpoint: u32, rank: i64) -> VertexId {
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&(endpoint as i64).to_be_bytes());
        data.extend_from_slice(&rank.to_be_bytes());
        VertexId::from_bytes(data)
    }

    pub(crate) fn decode_edge_endpoint(key: VertexId) -> (VertexId, i64) {
        let bytes = key.as_bytes();
        if bytes.len() != 16 {
            log::warn!(
                "decode_edge_endpoint: unexpected key length {}, expected 16",
                bytes.len()
            );
        }
        let mut buf = [0u8; 16];
        let copy_len = bytes.len().min(16);
        buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        let mut endpoint_bytes = [0u8; 8];
        endpoint_bytes.copy_from_slice(&buf[..8]);
        let mut rank_bytes = [0u8; 8];
        rank_bytes.copy_from_slice(&buf[8..16]);

        (
            VertexId::from_int64(i64::from_be_bytes(endpoint_bytes)),
            i64::from_be_bytes(rank_bytes),
        )
    }

    pub fn set_stats_manager(&mut self, stats: std::sync::Arc<crate::core::stats::StatsManager>) {
        self.stats_manager = Some(stats);
    }

    fn base_get_edge(
        &self,
        segments: &[CsrSegment],
        sparse_index: Option<&HashMap<u32, Vec<usize>>>,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
    ) -> Option<Nbr> {
        // Build relevant segment set for sparse index filtering
        let relevant_set: Option<std::collections::HashSet<usize>> = sparse_index
            .and_then(|idx| idx.get(&src))
            .map(|indices| indices.iter().copied().collect());

        // Scan segments in reverse (newest first), with early termination optimizations
        for (forward_idx, segment) in segments.iter().enumerate().rev() {
            // Sparse vertex index skip
            if let Some(ref set) = relevant_set {
                if !set.contains(&forward_idx) {
                    continue;
                }
            }
            // Skip segments that were created after the query timestamp
            if segment.create_ts_min > ts {
                continue;
            }

            // Fast path: skip segments where every edge has been deleted at
            // or before the query timestamp, so no entry in the segment can
            // be visible to this query. Both conditions are required: all
            // known deletions must predate the query AND the deleted count
            // must cover the whole segment. Checking only all_deleted_before
            // would drop the live edges of a partially deleted segment.
            if segment.deletion_info.all_deleted_before(ts)
                && segment
                    .deletion_info
                    .all_edges_deleted(segment.csr.read().edge_count())
            {
                continue;
            }

            // Ensure segment data is resident (reload from spill if evicted)
            if segment.is_evicted() {
                let _ = segment.reload_from_spill();
            }
            segment.record_access(GLOBAL_ACCESS_CLOCK.tick());

            // Try optimistic read first (seqlock-style, avoids RwLock contention).
            // If the segment is locked or the state changed during the read,
            // fall back to the RwLock read path.
            let positioned_edges = segment
                .try_optimistic_read(|csr| csr.edges_of_with_position(src))
                .unwrap_or_else(|| segment.csr.read().edges_of_with_position(src));

            for (position, edge) in positioned_edges {
                if edge.neighbor == dst && edge.timestamp <= ts {
                    let edge_id = segment.recover_edge_id(&edge, position);
                    if !self.mvcc.is_tombstoned(edge_id, ts) {
                        return Some(Nbr::new(
                            edge.neighbor,
                            edge_id,
                            edge.prop_offset,
                            edge.timestamp,
                        ));
                    }
                }
            }
        }

        None
    }

    fn base_edges_of(
        &self,
        segments: &[CsrSegment],
        sparse_index: Option<&HashMap<u32, Vec<usize>>>,
        src: u32,
        ts: Timestamp,
    ) -> Vec<Nbr> {
        let mut edges = Vec::new();

        // Build a set of segment indices that contain this vertex (for O(1) lookup)
        let relevant_set: Option<std::collections::HashSet<usize>> = sparse_index
            .and_then(|idx| idx.get(&src))
            .map(|indices| indices.iter().copied().collect());

        for (forward_idx, segment) in segments.iter().enumerate().rev() {
            // Sparse vertex index skip: if this segment does NOT contain the vertex, skip
            if let Some(ref set) = relevant_set {
                if !set.contains(&forward_idx) {
                    continue;
                }
            }

            if segment.create_ts_min > ts {
                continue;
            }

            // Skip segments where every edge has been deleted at or before the
            // query timestamp (no edge can be visible). Both conditions are
            // required: all known deletions predate the query AND the
            // deleted count covers the whole segment. all_deleted_before
            // alone is not sufficient, a partially deleted segment still
            // holds live edges.
            if segment.deletion_info.all_deleted_before(ts)
                && segment
                    .deletion_info
                    .all_edges_deleted(segment.csr.read().edge_count())
            {
                continue;
            }

            // Ensure segment data is resident (reload from spill if evicted)
            if segment.is_evicted() {
                let _ = segment.reload_from_spill();
            }
            segment.record_access(GLOBAL_ACCESS_CLOCK.tick());

            // Optimistic read with RwLock fallback
            let positioned_edges = segment
                .try_optimistic_read(|csr| csr.edges_of_with_position(src))
                .unwrap_or_else(|| segment.csr.read().edges_of_with_position(src));

            for (position, edge) in positioned_edges {
                if edge.timestamp <= ts {
                    let edge_id = segment.recover_edge_id(&edge, position);
                    if !self.mvcc.is_tombstoned(edge_id, ts) {
                        edges.push(Nbr::new(
                            edge.neighbor,
                            edge_id,
                            edge.prop_offset,
                            edge.timestamp,
                        ));
                    }
                }
            }
        }

        edges
    }

    fn merged_edges_of(
        &self,
        delta: &CsrVariant,
        segments: &[CsrSegment],
        sparse_index: Option<&HashMap<u32, Vec<usize>>>,
        src: u32,
        ts: Timestamp,
    ) -> Vec<Nbr> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        if let Some(iter) = delta.iter_edges_of(src, ts) {
            for nbr in iter {
                if !self.mvcc.is_tombstoned(nbr.edge_id, ts) && seen.insert(nbr.edge_id) {
                    result.push(*nbr);
                }
            }
        } else {
            for nbr in delta.edges_of(src, ts) {
                if !self.mvcc.is_tombstoned(nbr.edge_id, ts) && seen.insert(nbr.edge_id) {
                    result.push(nbr);
                }
            }
        }

        for nbr in self.base_edges_of(segments, sparse_index, src, ts) {
            if seen.insert(nbr.edge_id) {
                result.push(nbr);
            }
        }

        result
    }

    fn merged_get_edge(
        &self,
        delta: &CsrVariant,
        segments: &[CsrSegment],
        sparse_index: Option<&HashMap<u32, Vec<usize>>>,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
    ) -> Option<Nbr> {
        if let Some(nbr) = delta.get_edge(src, dst, ts) {
            if !self.mvcc.is_tombstoned(nbr.edge_id, ts) {
                return Some(nbr);
            }
        }

        self.base_get_edge(segments, sparse_index, src, dst, ts)
    }

    fn edge_record_from_nbr(&self, src: u32, nbr: Nbr, query_ts: Timestamp) -> EdgeRecord {
        let (dst_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
        EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid,
            rank,
            properties: self.properties_for_offset(nbr.prop_offset, query_ts),
        }
    }

    fn properties_for_offset(&self, prop_offset: u32, query_ts: Timestamp) -> Vec<(String, Value)> {
        if prop_offset == 0 {
            return Vec::new();
        }

        self.properties
            .get(prop_offset, Some(query_ts))
            .map(|props| {
                props
                    .into_iter()
                    .filter_map(|(k, v)| v.map(|v| (k, v)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn validate_segment_integrity(&self) -> usize {
        let mut valid_count = 0;

        for segment in &self.out_segments {
            if segment.version.validate(segment) {
                valid_count += 1;
            }
        }

        for segment in &self.in_segments {
            if segment.version.validate(segment) {
                valid_count += 1;
            }
        }

        valid_count
    }

    pub fn segment_versions(&self) -> Vec<(usize, u32)> {
        let mut versions = Vec::new();

        for (idx, seg) in self.out_segments.iter().enumerate() {
            versions.push((idx, seg.version.checksum));
        }

        for (idx, seg) in self.in_segments.iter().enumerate() {
            versions.push((idx + 1000, seg.version.checksum));
        }

        versions
    }

    pub fn update_segment_checksums(&mut self) {
        for segment in &mut self.out_segments {
            segment.version.checksum = SegmentVersion::compute_checksum(segment);
        }

        for segment in &mut self.in_segments {
            segment.version.checksum = SegmentVersion::compute_checksum(segment);
        }
    }

    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self.schema.oe_strategy == super::super::EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }

        let mut converted_values: Vec<(String, Value)> = Vec::with_capacity(property_values.len());
        for (name, value) in property_values {
            let prop_idx = self
                .property_index_cache
                .get(name)
                .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
            let prop_def = &self.schema.properties[*prop_idx];

            if value.data_type() != prop_def.data_type {
                let converted = value.try_cast_to(&prop_def.data_type)?;
                converted_values.push((name.clone(), converted));
            } else {
                converted_values.push((name.clone(), value.clone()));
            }
        }

        let prop_offset = if !converted_values.is_empty() {
            self.properties.insert(&converted_values, ts)?
        } else {
            0
        };

        if self.has_edge(src, dst, rank, ts) {
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(StorageError::edge_already_exists(format!(
                "{} -> {}@{}",
                src, dst, rank
            )));
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);

        let edge_id = self.next_edge_id.fetch_add();
        if let Err(e) = self
            .out_csr
            .insert_edge(src, dst_key, edge_id, prop_offset, ts)
        {
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(e);
        }

        if let Err(e) = self
            .in_csr
            .insert_edge(dst, src_key, edge_id, prop_offset, ts)
        {
            // Roll back the out-direction insertion physically so no
            // tombstone residue remains; fall back to logical deletion if
            // the entry cannot be located (e.g. strategy mismatch).
            if !self.out_csr.remove_edge(src, edge_id) {
                let _ = self.out_csr.delete_edge(src, edge_id, ts);
            }
            if prop_offset > 0 {
                self.properties.delete(prop_offset);
            }
            return Err(e);
        }

        // Update property index if enabled
        if let Some(ref mut index) = self.property_index {
            for (prop_name, prop_value) in &converted_values {
                let _ = index.insert(prop_name, prop_value, src, dst, rank, self.label, ts);
            }
        }

        // Check write backpressure after successful insertion
        self.check_and_apply_write_backpressure(ts);
        self.maybe_run_auto_maintenance();

        Ok(())
    }

    pub fn delete_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);

        // Look up edge properties before deletion for index maintenance
        let edge_properties = if self.property_index.is_some() {
            self.get_edge(src, dst, rank, ts).map(|e| e.properties)
        } else {
            None
        };

        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
            let edge_id = nbr.edge_id;

            if !self.out_csr.delete_edge(src, edge_id, ts)? {
                // Defensive: the out side could not be deleted.
                return Ok(false);
            }
            if !self.in_csr.delete_edge_by_dst(dst, src_key, ts) {
                // Roll back the out-direction deletion to keep both sides
                // consistent.
                self.out_csr.revert_delete_by_edge_id(src, edge_id, ts);
                return Ok(false);
            }

            // Mark the property record deleted once both sides are gone so
            // the row is reclaimable by compact_properties.
            if nbr.prop_offset > 0 {
                let _ = self.properties.mark_deleted(nbr.prop_offset, ts);
            }
            self.update_property_index_on_delete(&edge_properties, src, dst, rank, ts);
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        if let Some(nbr) = self.base_get_edge(
            &self.out_segments,
            Some(&self.sparse_vertex_index_out),
            src,
            dst_key,
            ts,
        ) {
            let edge_id = nbr.edge_id;
            self.mvcc.record_deletion(edge_id, ts);
            // Invalidate the cached current snapshot: it still contains this
            // edge and is only rebuilt lazily on the next maintenance pass.
            self.snapshot_dirty = true;
            // Mark the property record deleted so it can be reclaimed by
            // compact_properties (it filters via mvcc.is_tombstoned).
            if nbr.prop_offset > 0 {
                let _ = self.properties.mark_deleted(nbr.prop_offset, ts);
            }
            self.update_property_index_on_delete(&edge_properties, src, dst, rank, ts);
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    fn update_property_index_on_delete(
        &mut self,
        properties: &Option<Vec<(String, Value)>>,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) {
        if let Some(ref mut index) = self.property_index {
            if let Some(ref props) = properties {
                for (prop_name, prop_value) in props {
                    let _ = index.delete(prop_name, prop_value, src, dst, rank, ts);
                }
            }
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
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
            if !self.out_csr.delete_edge_by_offset(src, oe_offset, ts) {
                return Ok(false);
            }
            if !self.in_csr.delete_edge_by_offset(dst, ie_offset, ts) {
                // Roll back the out-direction deletion to keep both sides
                // consistent.
                self.out_csr.revert_delete_by_offset(src, oe_offset, ts);
                return Ok(false);
            }
            // Mark the property record deleted once both sides are gone so
            // the row is reclaimable by compact_properties.
            if nbr.prop_offset > 0 {
                let _ = self.properties.mark_deleted(nbr.prop_offset, ts);
            }
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }
        Ok(false)
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
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let reverted = self.out_csr.revert_delete_by_offset(src, oe_offset, ts);

        if reverted {
            self.in_csr.revert_delete_by_offset(dst, ie_offset, ts);
            // Restore the property record marked by mark_deleted so the edge
            // regains its original properties after the undo.
            let dst_key = Self::edge_endpoint_key(dst, rank);
            if let Some(nbr) = self.out_csr.get_edge(src, dst_key, ts) {
                if nbr.prop_offset > 0 {
                    self.properties.revert_deletion(nbr.prop_offset);
                }
            }
            return Ok(true);
        }

        // Segment-path undo: a frozen-segment deletion recorded an MVCC
        // tombstone plus a property-row mark (no CSR entry to revert). Undo it
        // by removing the tombstone and restoring the property row.
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let Some(nbr) = self.segment_find_edge_any(src, dst_key) else {
            return Ok(false);
        };
        // Only revert our own deletion: the tombstone must not be newer than
        // this undo point.
        match self.mvcc.delete_ts_of(nbr.edge_id) {
            Some(delete_ts) if delete_ts <= ts => {}
            _ => return Ok(false),
        }
        self.mvcc.remove_deletion(nbr.edge_id);
        // The cached current snapshot still excludes this edge; rebuild lazily.
        self.snapshot_dirty = true;
        if nbr.prop_offset > 0 {
            self.properties.revert_deletion(nbr.prop_offset);
        }
        // Re-index the restored properties when the property index is active
        // (the delete path removed them).
        let restored = self.properties_for_offset(nbr.prop_offset, ts);
        if let Some(ref mut index) = self.property_index {
            for (prop_name, prop_value) in restored {
                let _ = index.insert(&prop_name, &prop_value, src, dst, rank, self.label, ts);
            }
        }
        Ok(true)
    }

    /// Locate an edge in the frozen segments ignoring MVCC tombstones.
    ///
    /// Unlike [`Self::base_get_edge`] this returns entries whose deletion is
    /// already recorded — exactly what the segment-path delete undo needs.
    fn segment_find_edge_any(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        for segment in self.out_segments.iter() {
            if segment.is_evicted() {
                let _ = segment.reload_from_spill();
            }
            let positioned_edges = segment
                .try_optimistic_read(|csr| csr.edges_of_with_position(src))
                .unwrap_or_else(|| segment.csr.read().edges_of_with_position(src));
            for (position, edge) in positioned_edges {
                if edge.neighbor == dst {
                    let edge_id = segment.recover_edge_id(&edge, position);
                    return Some(Nbr::new(
                        edge.neighbor,
                        edge_id,
                        edge.prop_offset,
                        edge.timestamp,
                    ));
                }
            }
        }
        None
    }

    pub fn get_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.merged_get_edge(
            &self.out_csr,
            &self.out_segments,
            Some(&self.sparse_vertex_index_out),
            src,
            dst_key,
            ts,
        )?;
        let properties = self.properties_for_offset(nbr.prop_offset, ts);

        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs = self.merged_out_nbrs(src, ts);

        // Optimization: prefetch all properties first to improve cache locality
        let prop_offsets: Vec<_> = nbrs.iter().map(|nbr| nbr.prop_offset).collect();
        if !prop_offsets.is_empty() {
            self.properties.prefetch_batch(&prop_offsets);
        }

        nbrs.into_iter()
            .map(|nbr| {
                let (dst_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
                // Try fast path first, fall back to regular get if not fixed-size
                let properties = self
                    .properties
                    .get_fast(nbr.prop_offset, Some(ts))
                    .or_else(|| self.properties.get(nbr.prop_offset, Some(ts)))
                    .map(|props| {
                        props
                            .into_iter()
                            .filter_map(|(k, v)| v.map(|v| (k, v)))
                            .collect()
                    })
                    .unwrap_or_default();

                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid,
                    rank,
                    properties,
                }
            })
            .collect()
    }

    /// Raw out-edge neighbors of `src` (MVCC-merged, snapshot-consistent) with
    /// no property decoding.  Destination endpoint is encoded in `nbr.neighbor`.
    pub fn merged_out_nbrs(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        if ts == Timestamp::MAX && !self.snapshot_dirty && self.current_snapshot_out.is_some() {
            // Fast path: use current snapshot (single CSR lookup instead of per-segment iteration)
            self.merged_edges_of_current(&self.out_csr, src)
        } else {
            self.merged_edges_of(
                &self.out_csr,
                &self.out_segments,
                Some(&self.sparse_vertex_index_out),
                src,
                ts,
            )
        }
    }

    pub fn in_edges(&self, dst: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs = self.merged_in_nbrs(dst, ts);

        // Optimization: prefetch all properties first to improve cache locality
        let prop_offsets: Vec<_> = nbrs.iter().map(|nbr| nbr.prop_offset).collect();
        if !prop_offsets.is_empty() {
            self.properties.prefetch_batch(&prop_offsets);
        }

        nbrs.into_iter()
            .map(|nbr| {
                let (src_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
                // Try fast path first, fall back to regular get if not fixed-size
                let properties = self
                    .properties
                    .get_fast(nbr.prop_offset, Some(ts))
                    .or_else(|| self.properties.get(nbr.prop_offset, Some(ts)))
                    .map(|props| {
                        props
                            .into_iter()
                            .filter_map(|(k, v)| v.map(|v| (k, v)))
                            .collect()
                    })
                    .unwrap_or_default();

                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
    }

    /// Raw in-edge neighbors of `dst` (MVCC-merged, snapshot-consistent) with
    /// no property decoding.  Source endpoint is encoded in `nbr.neighbor`.
    pub fn merged_in_nbrs(&self, dst: u32, ts: Timestamp) -> Vec<Nbr> {
        if ts == Timestamp::MAX && !self.snapshot_dirty && self.current_snapshot_in.is_some() {
            self.merged_edges_of_current_in(&self.in_csr, dst)
        } else {
            self.merged_edges_of(
                &self.in_csr,
                &self.in_segments,
                Some(&self.sparse_vertex_index_in),
                dst,
                ts,
            )
        }
    }

    pub fn has_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        if !self.is_open {
            return false;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.merged_get_edge(
            &self.out_csr,
            &self.out_segments,
            Some(&self.sparse_vertex_index_out),
            src,
            dst_key,
            ts,
        )
        .is_some()
    }

    pub fn edge_count(&self) -> u64 {
        self.out_csr.edge_count()
            + self
                .out_segments
                .iter()
                .map(|segment| {
                    segment
                        .csr
                        .read()
                        .iter()
                        .filter(|(_, edge)| !self.mvcc.is_tombstoned(edge.edge_id, Timestamp::MAX))
                        .count() as u64
                })
                .sum::<u64>()
    }

    pub fn delta_edge_count(&self) -> u64 {
        self.out_csr.edge_count() + self.in_csr.edge_count()
    }

    pub fn scan(&self, ts: Timestamp) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        self.iter(ts).collect()
    }

    // ── OLAP: column pruning + zone-map pruning ──

    /// OLAP scan with column pruning: only `projection` columns are decoded
    /// (others are skipped), reducing IO 5-10x for wide edge tables. When
    /// `projection` is empty, all columns are returned. This is the columnar
    /// fast path for `MATCH ()-[r:TYPE]->() RETURN r.prop` style queries.
    #[allow(dead_code)]
    pub fn scan_projected(&self, ts: Timestamp, projection: &[String]) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        if projection.is_empty() {
            return self.scan(ts);
        }
        // Use iter for adjacency, but decode only projected columns.
        self.iter(ts)
            .map(|mut rec| {
                // Filter properties to projection (column pruning).
                rec.properties.retain(|(name, _)| projection.contains(name));
                rec
            })
            .collect()
    }

    /// OLAP `out_edges` with column pruning (zero-copy columnar path).
    /// Reads only the requested columns via `PropertyTable::get_projected`,
    /// which hits the `ColumnStore` per-column path instead of deserializing
    /// the whole row blob. Also benefits from zone-map pruning via
    /// `prune_by_zone_map` when a predicate range is supplied.
    #[allow(dead_code)]
    pub fn out_edges_projected(
        &self,
        src: u32,
        ts: Timestamp,
        projection: &[String],
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        let nbrs = self.merged_out_nbrs(src, ts);
        if nbrs.is_empty() {
            return Vec::new();
        }
        if projection.is_empty() {
            return self.out_edges(src, ts);
        }
        nbrs.into_iter()
            .map(|nbr| {
                let (dst_vid, rank) = Self::decode_edge_endpoint(nbr.neighbor);
                let properties = self
                    .properties
                    .get_projected(nbr.prop_offset, projection, Some(ts))
                    .map(|props| {
                        props
                            .into_iter()
                            .filter_map(|(k, v)| v.map(|v| (k, v)))
                            .collect()
                    })
                    .unwrap_or_default();
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid,
                    rank,
                    properties,
                }
            })
            .collect()
    }

    /// Zone-map predicate pruning helper: given a column and value range,
    /// returns which `ZONE_MAP_CHUNK_SIZE` chunks may contain matching rows.
    /// Callers can skip scanning chunks where `pruned[i] == false`.
    #[allow(dead_code)]
    pub fn prune_by_zone_map(
        &self,
        column: &str,
        lower: Option<&Value>,
        upper: Option<&Value>,
        include_lower: bool,
        include_upper: bool,
    ) -> Option<Vec<bool>> {
        self.properties
            .prune_chunks_by_range(column, lower, upper, include_lower, include_upper)
    }

    /// Expose per-column zone maps for optimizer CBO and ShowStats.
    #[allow(dead_code)]
    pub fn zone_map_for_column(
        &self,
        column: &str,
    ) -> Option<Vec<crate::storage::column_stats::ColumnStats>> {
        self.properties
            .zone_map_for_column(column)
            .map(|s| s.to_vec())
    }

    /// Per-column `ColumnStats` (global, aggregated from zone maps) for
    /// `ShowStats` and CBO cardinality estimation.
    #[allow(dead_code)]
    pub fn column_stats(
        &self,
        col_idx: usize,
    ) -> Option<crate::storage::column_stats::ColumnStats> {
        self.properties.compute_column_stats(col_idx)
    }

    /// Apply per-column compression encoding (ALP / bitpacking / dict / FSST / RLE)
    /// for OLAP IO reduction. Mirrors vertex `ColumnStore` encodings.
    #[allow(dead_code)]
    pub fn apply_column_encoding(
        &mut self,
        col_name: &str,
        encoding: crate::storage::encoding::EncodingType,
    ) -> StorageResult<()> {
        self.properties.apply_column_encoding(col_name, encoding)
    }

    /// Record a schema change event
    ///
    /// Handles the common pattern of:
    /// 1. Computing next version number from history
    /// 2. Creating a PropertyChange event
    /// 3. Recording it in the version history
    fn record_schema_change(&mut self, details: ChangeDetails) -> StorageResult<()> {
        // Get the next version number from history
        let mut history_guard = self
            .version_history
            .lock()
            .map_err(|_| StorageError::db_error("Failed to lock version_history"))?;

        let next_version = history_guard.latest_version() + 1;
        self.schema.schema_version = next_version;

        let change = PropertyChange::new(
            next_version,
            SchemaObjectType::Edge,
            self.label,
            self.label_name.clone(),
            details,
        );

        history_guard.add_change(change);

        Ok(())
    }

    pub fn add_property(
        &mut self,
        name: String,
        data_type: DataType,
        nullable: bool,
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self.properties.has_property(&name) {
            return Err(StorageError::column_already_exists(name));
        }

        self.properties
            .add_property(name.clone(), data_type.clone(), nullable)?;

        let prop_def = StoragePropertyDef::new(name.clone(), data_type.clone());
        let new_idx = self.schema.properties.len();
        self.schema.properties.push(prop_def);
        self.property_index_cache.insert(name.clone(), new_idx);

        self.record_schema_change(ChangeDetails::PropertyAdded {
            name,
            data_type,
            nullable,
            default_value: None,
        })?;

        Ok(())
    }

    /// Rebuild schema change record during WAL recovery
    ///
    /// This is used during recovery when the column already exists (from SchemaManager),
    /// but we need to update version_history to reflect the schema operation in the WAL.
    /// Does NOT add the property (it already exists), but DOES record the change.
    pub fn rebuild_schema_change_from_redo(&mut self, details: ChangeDetails) -> StorageResult<()> {
        self.record_schema_change(details)
    }

    pub fn remove_property(&mut self, name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;

        // Get property details before removal for change recording
        let removed_prop = self.schema.properties[index].clone();

        // Remove from properties first (potentially failing operation)
        self.properties.remove_property(name)?;
        // Only modify schema if properties removal succeeded
        self.schema.properties.remove(index);
        // Update cache: remove deleted property and adjust indices
        self.property_index_cache.remove(name);
        for idx in self.property_index_cache.values_mut() {
            if *idx > index {
                *idx -= 1;
            }
        }

        self.record_schema_change(ChangeDetails::PropertyRemoved {
            name: removed_prop.name,
            data_type: removed_prop.data_type,
        })?;

        Ok(())
    }

    pub fn rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self
            .schema
            .properties
            .iter()
            .any(|prop| prop.name == new_name)
        {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }

        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == old_name)
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;

        // Rename in properties first (potentially failing operation)
        self.properties.rename_property(old_name, new_name)?;
        // Only modify schema if properties rename succeeded
        self.schema.properties[index].name = new_name.to_string();
        // Update cache: rename key, keep index
        if let Some(idx) = self.property_index_cache.remove(old_name) {
            self.property_index_cache.insert(new_name.to_string(), idx);
        }

        self.record_schema_change(ChangeDetails::PropertyRenamed {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        })?;

        Ok(())
    }

    pub fn update_edge_property(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        // Validate property exists via cache
        let _ = self
            .property_index_cache
            .get(prop_name)
            .ok_or_else(|| StorageError::column_not_found(prop_name.to_string()))?;

        let dst_key = Self::edge_endpoint_key(dst, rank);
        if let Some(nbr) = self.merged_get_edge(
            &self.out_csr,
            &self.out_segments,
            Some(&self.sparse_vertex_index_out),
            src,
            dst_key,
            ts,
        ) {
            self.properties
                .set_property(nbr.prop_offset, prop_name, Some(value.clone()), ts)?;
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    pub fn update_edge_property_by_offset(
        &mut self,
        params: UpdateEdgePropertyByOffsetParams,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let dst_key = Self::edge_endpoint_key(params.dst, params.rank);
        if let Some(nbr) = self.merged_get_edge(
            &self.out_csr,
            &self.out_segments,
            Some(&self.sparse_vertex_index_out),
            params.src,
            dst_key,
            params.ts,
        ) {
            self.properties.set_property_by_id(
                nbr.prop_offset,
                PropertyId(params.prop_id),
                Some(params.value.clone()),
                params.ts,
            )?;

            let src_key = Self::edge_endpoint_key(params.src, params.rank);
            if let Some(ie_nbr) = self.merged_get_edge(
                &self.in_csr,
                &self.in_segments,
                Some(&self.sparse_vertex_index_in),
                params.dst,
                src_key,
                params.ts,
            ) {
                if nbr.prop_offset != ie_nbr.prop_offset {
                    return Err(StorageError::data_corruption(format!(
                        "property offset mismatch: out_csr={}, in_csr={} at edge ({}, {})",
                        nbr.prop_offset, ie_nbr.prop_offset, params.src, params.dst
                    )));
                }
            }
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    pub fn label(&self) -> LabelId {
        self.label
    }

    pub fn src_label(&self) -> LabelId {
        self.src_label
    }

    pub fn dst_label(&self) -> LabelId {
        self.dst_label
    }

    pub fn schema(&self) -> &EdgeSchema {
        &self.schema
    }

    pub(crate) fn schema_mut(&mut self) -> &mut EdgeSchema {
        &mut self.schema
    }

    pub fn set_schema(&mut self, schema: EdgeSchema) {
        // Rebuild property index cache
        self.property_index_cache.clear();
        for (idx, prop) in schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
        self.schema = schema;
    }

    /// Get reference to version history Arc for shared access
    pub fn version_history_ref(&self) -> Arc<Mutex<LabelVersionHistory>> {
        Arc::clone(&self.version_history)
    }

    pub fn iter(&self, ts: Timestamp) -> EdgeTableScanIterator<'_> {
        EdgeTableScanIterator::new(self, ts)
    }

    pub fn memory_size(&self) -> usize {
        let total = self.used_memory_size();
        let mutable = self.mutable_csr_memory_size();
        let out_epv = self.out_csr.edges_per_vertex();
        let in_epv = self.in_csr.edges_per_vertex();
        if out_epv > 0 || in_epv > 0 {
            log::trace!(
                "EdgeTable[{}] memory: {} bytes (mutable={}), MultiSingle edges_per_vertex (out={}, in={})",
                self.label,
                total,
                mutable,
                out_epv,
                in_epv
            );
        }
        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = 0;

        total += self.out_csr.used_memory_size();
        total += self.in_csr.used_memory_size();
        total += self
            .out_segments
            .iter()
            .map(|segment| segment.csr.read().used_memory_size())
            .sum::<usize>();
        total += self
            .in_segments
            .iter()
            .map(|segment| segment.csr.read().used_memory_size())
            .sum::<usize>();
        total += self.mvcc.total_tombstone_count() * std::mem::size_of::<(EdgeId, Timestamp)>();
        total += self.properties.used_memory_size();

        // Account for property_index_cache
        total += self.property_index_cache.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        // Account for the edge property index (if enabled)
        if let Some(ref index) = self.property_index {
            total += index.memory_usage() as usize;
        }

        total
    }

    /// Get mutable CSR memory usage (out_csr + in_csr)
    pub fn mutable_csr_memory_size(&self) -> usize {
        self.out_csr.used_memory_size() + self.in_csr.used_memory_size()
    }

    /// Estimate memory usage based on edge count and CSR strategy.
    /// This provides a more accurate estimate than used_memory_size() for freeze decisions.
    pub fn estimate_memory_usage(&self) -> usize {
        let out_edges = self.out_csr.edge_count() as usize;
        let in_edges = self.in_csr.edge_count() as usize;
        let out_bytes_per_edge = self.out_csr.bytes_per_edge();
        let in_bytes_per_edge = self.in_csr.bytes_per_edge();
        let estimated = out_edges * out_bytes_per_edge + in_edges * in_bytes_per_edge;

        let total_capacity = out_edges + in_edges;
        let frag_stats =
            crate::storage::edge::FragmentationStats::new(total_capacity, out_edges.min(in_edges));
        if frag_stats.fragmentation_ratio() > 2.0 {
            log::debug!(
                "EdgeTable[{}] high fragmentation: {:.2}",
                self.label,
                frag_stats.fragmentation_ratio()
            );
        }

        estimated
    }

    // ── Sparse vertex index methods ──

    /// Rebuild sparse vertex indices from scratch for both directions.
    /// Scans all segments to identify which vertices have edges in each segment.
    pub fn rebuild_sparse_vertex_indices(&mut self) {
        self.sparse_vertex_index_out.clear();
        for (seg_idx, seg) in self.out_segments.iter().enumerate() {
            let csr = seg.csr.read();
            for (src_vid, _) in csr.iter() {
                if let Some(vid) = src_vid.as_int64() {
                    self.sparse_vertex_index_out
                        .entry(vid as u32)
                        .or_default()
                        .push(seg_idx);
                }
            }
        }
        // Deduplicate segment indices per vertex
        for indices in self.sparse_vertex_index_out.values_mut() {
            indices.sort_unstable();
            indices.dedup();
        }

        self.sparse_vertex_index_in.clear();
        for (seg_idx, seg) in self.in_segments.iter().enumerate() {
            let csr = seg.csr.read();
            for (src_vid, _) in csr.iter() {
                if let Some(vid) = src_vid.as_int64() {
                    self.sparse_vertex_index_in
                        .entry(vid as u32)
                        .or_default()
                        .push(seg_idx);
                }
            }
        }
        for indices in self.sparse_vertex_index_in.values_mut() {
            indices.sort_unstable();
            indices.dedup();
        }
    }

    // ── Current snapshot methods ──

    /// Rebuild current snapshots from segments (eager rebuild).
    /// Called after freeze or merge operations when segments have changed.
    /// Rebuilds both out and in direction snapshots.
    pub fn rebuild_current_snapshot(&mut self) {
        // Build snapshot for out direction
        if !self.out_segments.is_empty() {
            use super::snapshot::SnapshotBuilder;
            let ts = Timestamp::MAX;
            let mut builder = SnapshotBuilder::new();
            for segment in self.out_segments.iter().rev() {
                builder.add_segment_edges(segment, ts, &self.mvcc.tombstones);
            }
            let edges = builder.edges();
            let vertex_capacity = self.out_csr.vertex_capacity();
            if let Ok(csr) = SnapshotBuilder::build_csr(edges, vertex_capacity) {
                self.current_snapshot_out = Some(csr);
            }
        } else {
            self.current_snapshot_out = None;
        }

        // Build snapshot for in direction
        if !self.in_segments.is_empty() {
            use super::snapshot::SnapshotBuilder;
            let ts = Timestamp::MAX;
            let mut builder = SnapshotBuilder::new();
            for segment in self.in_segments.iter().rev() {
                builder.add_segment_edges(segment, ts, &self.mvcc.tombstones);
            }
            let edges = builder.edges();
            let vertex_capacity = self.in_csr.vertex_capacity();
            if let Ok(csr) = SnapshotBuilder::build_csr(edges, vertex_capacity) {
                self.current_snapshot_in = Some(csr);
            }
        } else {
            self.current_snapshot_in = None;
        }

        self.snapshot_dirty = false;
    }

    /// Fast path for out_edges at ts=MAX: use current snapshot + mutable CSR,
    /// avoiding per-segment iteration.
    fn merged_edges_of_current(&self, delta: &CsrVariant, src: u32) -> Vec<Nbr> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        // 1. From mutable CSR
        if let Some(iter) = delta.iter_edges_of(src, Timestamp::MAX) {
            for nbr in iter {
                if !self.mvcc.is_tombstoned(nbr.edge_id, Timestamp::MAX) && seen.insert(nbr.edge_id)
                {
                    result.push(*nbr);
                }
            }
        } else {
            for nbr in delta.edges_of(src, Timestamp::MAX) {
                if !self.mvcc.is_tombstoned(nbr.edge_id, Timestamp::MAX) && seen.insert(nbr.edge_id)
                {
                    result.push(nbr);
                }
            }
        }

        // 2. From current snapshot (pre-merged segments, single CSR lookup)
        if let Some(ref snapshot) = self.current_snapshot_out {
            for edge in snapshot.edges_of(src).iter() {
                if !self.mvcc.is_tombstoned(edge.edge_id, Timestamp::MAX)
                    && seen.insert(edge.edge_id)
                {
                    result.push(Nbr::new(
                        edge.neighbor,
                        edge.edge_id,
                        edge.prop_offset,
                        edge.timestamp,
                    ));
                }
            }
        }

        result
    }

    /// Fast path for in_edges at ts=MAX.
    fn merged_edges_of_current_in(&self, delta: &CsrVariant, dst: u32) -> Vec<Nbr> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        if let Some(iter) = delta.iter_edges_of(dst, Timestamp::MAX) {
            for nbr in iter {
                if !self.mvcc.is_tombstoned(nbr.edge_id, Timestamp::MAX) && seen.insert(nbr.edge_id)
                {
                    result.push(*nbr);
                }
            }
        } else {
            for nbr in delta.edges_of(dst, Timestamp::MAX) {
                if !self.mvcc.is_tombstoned(nbr.edge_id, Timestamp::MAX) && seen.insert(nbr.edge_id)
                {
                    result.push(nbr);
                }
            }
        }

        if let Some(ref snapshot) = self.current_snapshot_in {
            for edge in snapshot.edges_of(dst).iter() {
                if !self.mvcc.is_tombstoned(edge.edge_id, Timestamp::MAX)
                    && seen.insert(edge.edge_id)
                {
                    result.push(Nbr::new(
                        edge.neighbor,
                        edge.edge_id,
                        edge.prop_offset,
                        edge.timestamp,
                    ));
                }
            }
        }

        result
    }

    /// Record mutable CSR pressure without performing maintenance on the write path.
    pub fn check_and_apply_write_backpressure(&mut self, _current_ts: Timestamp) -> bool {
        if self.config.max_mutable_csr_bytes == 0 {
            return false; // Backpressure disabled
        }

        let mutable_size = self.estimate_memory_usage();

        // Record current metrics
        if let Some(stats) = &self.stats_manager {
            stats.record_mutable_csr_backpressure(mutable_size as u64, mutable_size as u64);
        }

        if mutable_size > self.config.max_mutable_csr_bytes {
            return true;
        }

        false
    }

    pub fn needs_background_freeze(&self) -> bool {
        self.config.max_mutable_csr_bytes > 0
            && self.estimate_memory_usage() > self.config.max_mutable_csr_bytes
    }

    /// Run automatic maintenance based on configured thresholds.
    ///
    /// Called from write paths (`insert_edge`, `delete_edge`, updates) so
    /// deleted entries and stale metadata are reclaimed without waiting for
    /// an explicit maintenance invocation:
    ///
    /// - tombstone GC when the total tombstone count exceeds the threshold
    ///   (rate-limited by `gc_min_serial` to bound write-path latency)
    /// - property compaction when the deleted-row ratio is high
    /// - delta freeze when the mutable CSR exceeds its memory cap
    ///
    /// Returns the number of edges removed (0 if no maintenance ran).
    pub fn maybe_run_auto_maintenance(&mut self) -> usize {
        let cfg = self.config.auto_maintenance;
        if cfg.tombstone_gc_threshold == 0 && cfg.max_delta_memory_bytes == 0 {
            return 0;
        }
        let mut maintenance_ran = 0;

        // Tier 1: tombstone GC (rate-limited by serial counter).
        if cfg.tombstone_gc_threshold > 0
            && self.mvcc.total_tombstone_count() > cfg.tombstone_gc_threshold
        {
            let bound = self.mvcc.effective_retention_bound();
            if bound < Timestamp::MAX
                && (bound != self.last_gc_min_snapshot_ts
                    || (cfg.gc_min_serial > 0
                        && self.maintenance_serial.is_multiple_of(cfg.gc_min_serial)))
            {
                let cleaned = self.mvcc.gc_tombstones(bound);
                self.last_gc_min_snapshot_ts = bound;
                self.maintenance_serial = self.maintenance_serial.saturating_add(1);
                if cleaned > 0 {
                    maintenance_ran += 1;
                    log::debug!(
                        "Auto-maintenance GC: removed {} tombstones (bound={}, total={})",
                        cleaned,
                        bound,
                        self.mvcc.total_tombstone_count()
                    );
                }
            }
        }

        // Tier 2: property table compaction when the deleted-row ratio is high.
        // Only runs under a bounded retention horizon: compaction reclaims
        // version chains older than the bound, so without a bounded horizon
        // there is no known safe retention boundary (ad-hoc time-travel reads
        // may still need the history).
        let bound = self.mvcc.effective_retention_bound();
        if cfg.property_compact_ratio > 0.0 && bound != Timestamp::MAX {
            let prop_stats = self.properties.compaction_stats();
            if prop_stats.fragmentation_ratio() >= cfg.property_compact_ratio as f64 {
                self.compact_properties(bound);
                self.maintenance_serial = self.maintenance_serial.saturating_add(1);
                maintenance_ran += 1;
            }
        }

        // Tier 3: freeze delta when it exceeds its own memory cap (or the
        // global cap, whichever is lower).
        let freeze_cap = if cfg.max_delta_memory_bytes > 0 {
            cfg.max_delta_memory_bytes
        } else {
            self.config.max_mutable_csr_bytes
        };
        if freeze_cap > 0 && self.estimate_memory_usage() > freeze_cap {
            self.freeze_csr_only(Timestamp::MAX);
            self.maintenance_serial = self.maintenance_serial.saturating_add(1);
            maintenance_ran += 1;
        }

        // Tier 4: PhysicalDeletion merge when the tombstone pressure on frozen
        // segments is high. Edges are physically dropped only when a bounded
        // `min_active_snapshot_ts` exists (no snapshot can observe them);
        // without snapshots the merge keeps every edge.
        if cfg.deletion_compact_ratio > 0.0 {
            let del_stats = self.deletion_stats();
            let density = if del_stats.total_frozen_edges == 0 {
                0.0
            } else {
                self.mvcc.total_tombstone_count() as f64 / del_stats.total_frozen_edges as f64
            };
            if density >= cfg.deletion_compact_ratio {
                let bound = self.mvcc.effective_retention_bound();
                let merge_threshold = CompactConfig::default()
                    .compute_merge_size_threshold(self.mvcc.tombstone_stats().memory_bytes);
                let result = self.merge_segments_with_config_and_deletion_filter(
                    self.config.segment_merge_threshold as Timestamp,
                    merge_threshold,
                    if bound < Timestamp::MAX {
                        Some(bound)
                    } else {
                        None
                    },
                );
                if result.segments_reduced > 0 {
                    self.maintenance_serial = self.maintenance_serial.saturating_add(1);
                    maintenance_ran += 1;
                    log::debug!(
                        "Auto-maintenance physical merge reduced segments by {} (density={:.2})",
                        result.segments_reduced,
                        density
                    );
                }
            }
        }

        maintenance_ran
    }

    // ── Edge Property Index ──

    /// Enable property index with the specified pool capacity.
    /// Builds the index from existing edge data.
    pub fn enable_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        self.build_property_index(pool_capacity)
    }

    /// Build the property index by scanning all edges.
    pub(crate) fn build_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        let mut index = EdgePropertyIndex::new(pool_capacity);
        // MAX_TIMESTAMP (not INVALID_TIMESTAMP) satisfies `create_ts <= ts < delete_ts`
        // for live edges, so all non-tombstoned edges are scanned.
        let all_ts = crate::core::types::MAX_TIMESTAMP;

        let iter = EdgeTableScanIterator::new(self, all_ts);
        let edge_records: Vec<EdgeRecord> = iter.collect();
        for edge in &edge_records {
            let src_u32 = edge.src_vid.as_int64().unwrap_or(0) as u32;
            let dst_u32 = edge.dst_vid.as_int64().unwrap_or(0) as u32;
            for (prop_name, prop_value) in &edge.properties {
                let _ = index.insert(
                    prop_name, prop_value, src_u32, dst_u32, edge.rank, self.label, all_ts,
                );
            }
        }

        self.property_index = Some(index);
        Ok(())
    }

    /// Check if property index is enabled.
    pub fn has_property_index(&self) -> bool {
        self.property_index.is_some()
    }

    /// Drop the property index to free memory.
    pub fn disable_property_index(&mut self) {
        self.property_index = None;
    }

    /// Lookup edges by a property value range using the EdgePropertyIndex.
    /// Returns `(src, dst, rank)` tuples for matching edges.
    pub fn lookup_edges_by_property_range(
        &self,
        prop_name: &str,
        value_lower: &[u8],
        value_upper: &[u8],
    ) -> Vec<(u32, u32, i64)> {
        let Some(ref index) = self.property_index else {
            return Vec::new();
        };
        if !index.has_index(prop_name) {
            return Vec::new();
        }
        index
            .lookup(prop_name, value_lower, value_upper)
            .into_iter()
            .map(|((src, dst, rank), _record)| (src, dst, rank))
            .collect()
    }
}

pub struct EdgeTableScanIterator<'a> {
    _table: &'a TimeTravelEdgeStore,
    records: std::vec::IntoIter<EdgeRecord>,
    /// Maximum number of records to return (None = unlimited)
    max_records: Option<usize>,
    /// Current record count
    current_count: usize,
}

impl<'a> EdgeTableScanIterator<'a> {
    pub fn new(table: &'a TimeTravelEdgeStore, ts: Timestamp) -> Self {
        Self::with_limit(table, ts, None)
    }

    /// Create a scan iterator with a maximum record limit
    pub fn with_limit(
        table: &'a TimeTravelEdgeStore,
        ts: Timestamp,
        max_records: Option<usize>,
    ) -> Self {
        let mut seen = HashSet::new();
        let mut records = Vec::new();

        for (src_vid, nbr) in table.out_csr.iter(ts) {
            if !table.mvcc.is_tombstoned(nbr.edge_id, ts) && seen.insert(nbr.edge_id) {
                records.push(table.edge_record_from_nbr(
                    src_vid.as_int64().unwrap_or(0) as u32,
                    nbr,
                    ts,
                ));

                if let Some(max) = max_records {
                    if records.len() >= max {
                        break;
                    }
                }
            }
        }

        if records.len() < max_records.unwrap_or(usize::MAX) {
            for segment in table.out_segments.iter().rev() {
                if segment.create_ts_min > ts {
                    continue;
                }

                for (src_vid, edge) in segment.csr.read().iter() {
                    if edge.timestamp <= ts
                        && !table.mvcc.is_tombstoned(edge.edge_id, ts)
                        && seen.insert(edge.edge_id)
                    {
                        records.push(table.edge_record_from_nbr(
                            src_vid.as_int64().unwrap_or(0) as u32,
                            Nbr::new(
                                edge.neighbor,
                                edge.edge_id,
                                edge.prop_offset,
                                edge.timestamp,
                            ),
                            ts,
                        ));

                        if let Some(max) = max_records {
                            if records.len() >= max {
                                break;
                            }
                        }
                    }
                }

                if let Some(max) = max_records {
                    if records.len() >= max {
                        break;
                    }
                }
            }
        }

        Self {
            _table: table,
            records: records.into_iter(),
            max_records,
            current_count: 0,
        }
    }
}

impl<'a> Iterator for EdgeTableScanIterator<'a> {
    type Item = EdgeRecord;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(max) = self.max_records {
            if self.current_count >= max {
                return None;
            }
        }

        if let Some(record) = self.records.next() {
            self.current_count += 1;
            Some(record)
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
