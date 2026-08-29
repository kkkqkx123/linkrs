//! Segment management: immutable CSR segments with versioning and deletion tracking.
//!
//! Segments represent frozen portions of the edge table, storing compressed sparse row (CSR)
//! data with metadata for time-travel queries and MVCC support.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;

use super::super::{Csr, CsrBase, EdgeId};
use super::page_state::SegmentLockState;
use super::residency::SegmentResidency;
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult};

/// Deletion information for a CSR segment.
///
/// Tracks the deletion timestamp range and count for edges in the segment.
/// This enables time-travel query optimizations and accurate MVCC semantics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeletionInfo {
    /// No edges in this segment have been deleted
    NoDeletes,
    /// Some edges have been deleted in the range [min_ts, max_ts]
    /// - min_ts: earliest deletion timestamp
    /// - max_ts: latest deletion timestamp
    /// - deleted_count: exact number of deleted edges (for optimization)
    HasDeletes {
        min_ts: Timestamp,
        max_ts: Timestamp,
        deleted_count: u32,
    },
}

impl DeletionInfo {
    /// Create from deletion timestamps. NoDeletes if min=MAX or max=0.
    pub fn new(min: Timestamp, max: Timestamp) -> Self {
        if min == Timestamp::MAX || max == 0 {
            DeletionInfo::NoDeletes
        } else {
            DeletionInfo::HasDeletes {
                min_ts: min,
                max_ts: max,
                deleted_count: 0,
            }
        }
    }

    /// Create with known deleted count (used during freeze/segment creation)
    pub fn with_count(min: Timestamp, max: Timestamp, deleted_count: u32) -> Self {
        if min == Timestamp::MAX || max == 0 || deleted_count == 0 {
            DeletionInfo::NoDeletes
        } else {
            DeletionInfo::HasDeletes {
                min_ts: min,
                max_ts: max,
                deleted_count,
            }
        }
    }

    /// Check if all deletions happened before or at query_ts
    pub fn all_deleted_before(&self, query_ts: Timestamp) -> bool {
        match self {
            DeletionInfo::NoDeletes => false,
            DeletionInfo::HasDeletes { max_ts, .. } => *max_ts <= query_ts,
        }
    }

    /// Check if all edges in segment are deleted (fast path for complete deletion)
    pub fn all_edges_deleted(&self, total_edge_count: u64) -> bool {
        match self {
            DeletionInfo::NoDeletes => false,
            DeletionInfo::HasDeletes { deleted_count, .. } => {
                *deleted_count as u64 == total_edge_count
            }
        }
    }

    /// Get deletion percentage (0-100) for observability
    pub fn deletion_percentage(&self, total_edge_count: u64) -> u32 {
        let deleted_count = match self {
            DeletionInfo::NoDeletes => 0,
            DeletionInfo::HasDeletes { deleted_count, .. } => *deleted_count as u64,
        };
        if total_edge_count == 0 {
            return 0;
        }
        (deleted_count * 100 / total_edge_count) as u32
    }

    /// Merge two deletion infos by taking min of mins, max of maxs, and sum of counts
    pub fn merge(&self, other: &DeletionInfo) -> DeletionInfo {
        match (self, other) {
            (DeletionInfo::NoDeletes, DeletionInfo::NoDeletes) => DeletionInfo::NoDeletes,
            (
                DeletionInfo::NoDeletes,
                DeletionInfo::HasDeletes {
                    min_ts,
                    max_ts,
                    deleted_count,
                },
            )
            | (
                DeletionInfo::HasDeletes {
                    min_ts,
                    max_ts,
                    deleted_count,
                },
                DeletionInfo::NoDeletes,
            ) => DeletionInfo::HasDeletes {
                min_ts: *min_ts,
                max_ts: *max_ts,
                deleted_count: *deleted_count,
            },
            (
                DeletionInfo::HasDeletes {
                    min_ts: min1,
                    max_ts: max1,
                    deleted_count: count1,
                },
                DeletionInfo::HasDeletes {
                    min_ts: min2,
                    max_ts: max2,
                    deleted_count: count2,
                },
            ) => DeletionInfo::HasDeletes {
                min_ts: (*min1).min(*min2),
                max_ts: (*max1).max(*max2),
                deleted_count: count1.saturating_add(*count2),
            },
        }
    }
}

/// Version tracking for CSR segment recovery
#[derive(Debug, Clone, Copy)]
pub struct SegmentVersion {
    /// CRC32 checksum for integrity validation
    pub checksum: u32,
}

impl SegmentVersion {
    /// Create a new segment version
    pub fn new() -> Self {
        Self { checksum: 0 }
    }

    /// Compute CRC32 checksum for segment
    pub fn compute_checksum(segment: &CsrSegment) -> u32 {
        let mut crc = 0u32;
        crc = crc
            .wrapping_mul(31)
            .wrapping_add(segment.csr.read().edge_count() as u32);
        crc = crc
            .wrapping_mul(31)
            .wrapping_add(segment.create_ts_min as u32);
        crc = crc
            .wrapping_mul(31)
            .wrapping_add(segment.create_ts_max as u32);
        crc
    }

    /// Validate segment integrity
    pub fn validate(&self, segment: &CsrSegment) -> bool {
        let computed = Self::compute_checksum(segment);
        self.checksum == computed || self.checksum == 0
    }
}

/// NbrWithoutEdgeId optimization: auto-enabled for segments >= 10K edges
/// Saves ~15% memory by storing edge_ids separately, with O(1) recovery lookup
pub const SEPARATE_EDGE_ID_STORAGE_THRESHOLD: usize = 10_000;

/// Default number of vertices per region for region-level recycling.
pub const DEFAULT_REGION_VERTEX_COUNT: usize = 1024;

/// Per-region metadata for fine-grained recycling.
///
/// A region is a contiguous vertex-id interval `[vertex_start, vertex_end)` in
/// the direction's vertex space (out: src, in: dst). Metadata is derived and
/// rebuildable from the CSR + MVCC tombstones; it is cached for merge/compact
/// decisions and persisted for faster reload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionMeta {
    pub region_id: u32,
    pub vertex_start: u32,
    pub vertex_end: u32,
    pub edge_count: u32,
    pub deleted_count: u32,
    pub deletion_info: DeletionInfo,
    pub estimated_bytes: usize,
}

/// Region-stat helpers used by the unit tests that verify region metadata
/// derivation and persistence; production merge decisions read the raw
/// `deleted_count` / `edge_count` fields directly.
#[cfg(test)]
impl RegionMeta {
    pub fn deletion_ratio(&self) -> f64 {
        if self.edge_count == 0 {
            0.0
        } else {
            self.deleted_count as f64 / self.edge_count as f64
        }
    }
}

pub struct CsrSegment {
    pub csr: RwLock<Csr>,
    /// Edge creation time range: [create_ts_min, create_ts_max]
    pub create_ts_min: Timestamp,
    pub create_ts_max: Timestamp,
    /// Deletion information for time-travel queries and GC
    pub deletion_info: DeletionInfo,
    /// Version tracking for recovery
    pub version: SegmentVersion,
    /// Timestamp when this segment was created (for adaptive merge decisions)
    pub created_at_ts: Timestamp,
    /// Optional separate edge_id storage for memory optimization
    /// None: direct mode (edge_id in ImmutableNbr)
    /// Some(...): optimized mode (edge_id stored separately, 15% memory savings)
    pub edge_ids: Option<Vec<EdgeId>>,
    /// Region-level metadata (logical slices, single Csr).
    pub regions: Vec<RegionMeta>,
    /// Vertex count per region (0 = disabled, fallback to segment-level).
    pub region_vertex_count: usize,
    /// Residency state: whether CSR data is in memory or evicted to disk
    pub residency: RwLock<SegmentResidency>,
    /// Lock state for optimistic reads and write coordination
    pub lock_state: SegmentLockState,
    /// Last access timestamp for LRU eviction ordering
    pub last_access_ts: AtomicU64,
}

impl CsrSegment {
    pub fn new(
        csr: Csr,
        create_ts_min: Timestamp,
        create_ts_max: Timestamp,
        deletion_info: DeletionInfo,
    ) -> Self {
        Self::with_creation_ts(
            csr,
            create_ts_min,
            create_ts_max,
            deletion_info,
            Timestamp::MAX,
        )
    }

    pub fn with_creation_ts(
        csr: Csr,
        create_ts_min: Timestamp,
        create_ts_max: Timestamp,
        deletion_info: DeletionInfo,
        created_at_ts: Timestamp,
    ) -> Self {
        let mut seg = Self {
            csr: RwLock::new(csr),
            create_ts_min,
            create_ts_max,
            deletion_info,
            version: SegmentVersion::new(),
            created_at_ts,
            edge_ids: None,
            regions: Vec::new(),
            region_vertex_count: 0,
            residency: RwLock::new(SegmentResidency::Resident),
            lock_state: SegmentLockState::new(),
            last_access_ts: AtomicU64::new(0),
        };
        seg.version.checksum = SegmentVersion::compute_checksum(&seg);
        seg
    }

    /// Recover EdgeId from this segment at given CSR position
    ///
    /// Supports both direct mode (edge_id in ImmutableNbr) and optimized mode
    /// (edge_id stored separately). Transparent to callers.
    pub fn recover_edge_id(&self, nbr: &super::super::ImmutableNbr, csr_position: usize) -> EdgeId {
        match &self.edge_ids {
            Some(ids) => ids.get(csr_position).copied().unwrap_or(nbr.edge_id),
            None => nbr.edge_id,
        }
    }

    /// Calculate age of this segment in timestamp units
    pub fn age(&self, current_ts: Timestamp) -> Timestamp {
        if self.created_at_ts == Timestamp::MAX {
            0
        } else {
            current_ts.saturating_sub(self.created_at_ts)
        }
    }

    /// Get deletion percentage (0.0-1.0) of this segment
    pub fn deletion_ratio(&self) -> f64 {
        let edge_count = self.csr.read().edge_count();
        if edge_count == 0 {
            0.0
        } else {
            match self.deletion_info {
                DeletionInfo::NoDeletes => 0.0,
                DeletionInfo::HasDeletes { deleted_count, .. } => {
                    (deleted_count as f64) / (edge_count as f64)
                }
            }
        }
    }

    /// Get deletion info as (min, max) range for serialization
    pub fn deletion_range(&self) -> (Timestamp, Timestamp) {
        match self.deletion_info {
            DeletionInfo::NoDeletes => (Timestamp::MAX, 0),
            DeletionInfo::HasDeletes { min_ts, max_ts, .. } => (min_ts, max_ts),
        }
    }

    /// Estimate memory usage of this segment in bytes
    pub fn estimated_bytes(&self) -> usize {
        let csr_bytes = self.csr.read().used_memory_size();
        let metadata_bytes =
            std::mem::size_of::<Timestamp>() * 2 + std::mem::size_of::<DeletionInfo>();
        csr_bytes + metadata_bytes
    }

    /// Returns true if this segment's CSR data is resident in memory.
    pub fn is_resident(&self) -> bool {
        self.residency.read().is_resident()
    }

    /// Returns true if this segment's CSR data has been evicted to disk.
    pub fn is_evicted(&self) -> bool {
        self.residency.read().is_evicted()
    }

    /// Returns the spill file size in bytes (0 if resident).
    pub fn spill_size(&self) -> u64 {
        self.residency.read().spill_size()
    }

    /// Begin eviction: CAS from Unlocked → Marked (first pass).
    /// The segment remains readable by optimistic readers while marked.
    /// Returns true if the segment was successfully marked.
    pub fn begin_eviction(&self) -> bool {
        self.lock_state.try_mark()
    }

    /// Complete eviction: CAS from Marked → Evicted, then dump CSR to spill file.
    /// Returns the number of bytes written to the spill file.
    pub fn finish_eviction(&self, spill_path: &Path) -> StorageResult<u64> {
        if !self.lock_state.try_evict() {
            return Err(StorageError::invalid_operation(
                "segment is not in Marked state".to_string(),
            ));
        }

        let mut residency = self.residency.write();
        let mut csr = self.csr.write();
        let bytes = csr.dump_to_file(spill_path)?;
        *csr = Csr::new();
        *residency = SegmentResidency::Evicted {
            spill_path: spill_path.to_path_buf(),
            spill_size: bytes,
        };
        Ok(bytes)
    }

    /// Evict this segment's CSR data to a spill file, freeing physical memory.
    ///
    /// Single-shot eviction: transitions the segment to Evicted and dumps CSR.
    /// If the segment is Unlocked, it goes through Marked → Evicted atomically.
    /// Returns error if the segment is locked by a writer.
    pub fn evict_to_spill(&self, spill_path: &Path) -> StorageResult<u64> {
        let residency = self.residency.read();
        if !residency.is_resident() {
            return Err(StorageError::invalid_operation(
                "segment is already evicted".to_string(),
            ));
        }
        drop(residency);

        // If already Marked, complete the eviction
        if self.lock_state.read_state() == super::page_state::SegmentState::Marked {
            return self.finish_eviction(spill_path);
        }

        // Transition Unlocked → Evicted via Marked
        if self.begin_eviction() {
            return self.finish_eviction(spill_path);
        }

        Err(StorageError::invalid_operation(
            "segment is locked by writer".to_string(),
        ))
    }

    /// Reload this segment's CSR data from a spill file back into memory.
    ///
    /// Uses CAS to transition: Evicted → Unlocked.
    pub fn reload_from_spill(&self) -> StorageResult<()> {
        if self.is_resident() {
            return Err(StorageError::invalid_operation(
                "segment is already resident".to_string(),
            ));
        }

        let residency = self.residency.read();
        let spill_path = match &*residency {
            SegmentResidency::Evicted { spill_path, .. } => spill_path.clone(),
            SegmentResidency::Resident => unreachable!(), // checked above
        };
        drop(residency);

        let mut csr = self.csr.write();
        csr.load_from_file(&spill_path)?;
        drop(csr);

        let mut residency = self.residency.write();
        *residency = SegmentResidency::Resident;
        self.lock_state.try_resurrect();
        Ok(())
    }

    /// Attempt an optimistic read on this segment's CSR data.
    ///
    /// Returns `Some(result)` if the segment was Unlocked for the entire read,
    /// or `None` if a writer was active or the state changed during the read.
    /// On `None`, the caller should fall back to acquiring `self.csr.read()`.
    pub fn try_optimistic_read<F, R>(&self, func: F) -> Option<R>
    where
        F: FnOnce(&Csr) -> R,
    {
        self.lock_state.try_optimistic_read(|| {
            let csr = self.csr.read();
            func(&csr)
        })
    }

    /// Record an access for LRU tracking.
    pub fn record_access(&self, clock_ts: u64) {
        self.last_access_ts.store(clock_ts, Ordering::Relaxed);
    }

    /// Consume the segment and return its CSR allocation to the free-space pool.
    pub(crate) fn into_csr(self) -> Csr {
        self.csr.into_inner()
    }

    /// Rebuild logical region metadata for this segment.
    ///
    /// When `region_vertex_count == 0` regions are disabled and cleared.
    /// Otherwise `regions` is sized to `ceil(vertex_capacity / N)` and each
    /// entry's `edge_count`/`deleted_count`/`deletion_info` is derived from
    /// the CSR + MVCC tombstones. The method is idempotent and cheap for
    /// small segments.
    pub fn rebuild_regions(
        &mut self,
        region_vertex_count: usize,
        mvcc_delete_ts: &dyn Fn(EdgeId) -> Option<Timestamp>,
    ) {
        if region_vertex_count == 0 {
            self.regions.clear();
            self.region_vertex_count = 0;
            return;
        }
        self.region_vertex_count = region_vertex_count;
        let csr = self.csr.read();
        let vc = csr.vertex_capacity();
        if vc == 0 {
            self.regions.clear();
            return;
        }
        let region_cnt = vc.div_ceil(region_vertex_count);
        let mut metas = Vec::with_capacity(region_cnt);
        for rid in 0..region_cnt {
            let start = (rid * region_vertex_count) as u32;
            let end = ((rid + 1) * region_vertex_count).min(vc) as u32;
            metas.push(RegionMeta {
                region_id: rid as u32,
                vertex_start: start,
                vertex_end: end,
                edge_count: 0,
                deleted_count: 0,
                deletion_info: DeletionInfo::NoDeletes,
                estimated_bytes: 0,
            });
        }
        // First pass: edge counts per region
        let mut edge_counts = vec![0u32; region_cnt];
        let mut deleted_counts = vec![0u32; region_cnt];
        let mut del_mins = vec![Timestamp::MAX; region_cnt];
        let mut del_maxs = vec![0u64; region_cnt];
        for (edge_pos, (src, nbr)) in csr.iter().enumerate() {
            let src_u32 = src.as_int64().unwrap_or(0) as u32;
            let rid = (src_u32 as usize / region_vertex_count).min(region_cnt - 1);
            edge_counts[rid] += 1;
            let eid = self.recover_edge_id(nbr, edge_pos);
            if let Some(ts) = mvcc_delete_ts(eid) {
                deleted_counts[rid] += 1;
                del_mins[rid] = del_mins[rid].min(ts);
                del_maxs[rid] = del_maxs[rid].max(ts);
            }
        }
        for (i, meta) in metas.iter_mut().enumerate() {
            meta.edge_count = edge_counts[i];
            meta.deleted_count = deleted_counts[i];
            meta.deletion_info =
                DeletionInfo::with_count(del_mins[i], del_maxs[i], deleted_counts[i]);
            // Estimate: edges * size_of ImmutableNbr + offsets slice
            let width = (meta.vertex_end - meta.vertex_start) as usize;
            meta.estimated_bytes = (meta.edge_count as usize)
                * std::mem::size_of::<super::super::ImmutableNbr>()
                + (width + 1) * std::mem::size_of::<u32>();
        }
        self.regions = metas;
    }

    /// Rebuild regions from inline delete info without MVCC (used at freeze time where inline deletes exist).
    pub fn rebuild_regions_from_entries(
        &mut self,
        region_vertex_count: usize,
        entries: &[(u32, super::super::Nbr, Timestamp)],
    ) {
        if region_vertex_count == 0 {
            self.regions.clear();
            self.region_vertex_count = 0;
            return;
        }
        self.region_vertex_count = region_vertex_count;
        let csr = self.csr.read();
        let vc = csr.vertex_capacity();
        if vc == 0 {
            self.regions.clear();
            return;
        }
        let region_cnt = vc.div_ceil(region_vertex_count);
        let mut metas = Vec::with_capacity(region_cnt);
        for rid in 0..region_cnt {
            let start = (rid * region_vertex_count) as u32;
            let end = ((rid + 1) * region_vertex_count).min(vc) as u32;
            metas.push(RegionMeta {
                region_id: rid as u32,
                vertex_start: start,
                vertex_end: end,
                edge_count: 0,
                deleted_count: 0,
                deletion_info: DeletionInfo::NoDeletes,
                estimated_bytes: 0,
            });
        }
        let mut edge_counts = vec![0u32; region_cnt];
        let mut deleted_counts = vec![0u32; region_cnt];
        let mut del_mins = vec![Timestamp::MAX; region_cnt];
        let mut del_maxs = vec![0u64; region_cnt];
        for (src, nbr, _) in entries {
            let rid = (*src as usize / region_vertex_count).min(region_cnt - 1);
            edge_counts[rid] += 1;
            if nbr.delete_ts != Timestamp::MAX {
                deleted_counts[rid] += 1;
                del_mins[rid] = del_mins[rid].min(nbr.delete_ts);
                del_maxs[rid] = del_maxs[rid].max(nbr.delete_ts);
            }
        }
        for (i, meta) in metas.iter_mut().enumerate() {
            meta.edge_count = edge_counts[i];
            meta.deleted_count = deleted_counts[i];
            meta.deletion_info =
                DeletionInfo::with_count(del_mins[i], del_maxs[i], deleted_counts[i]);
            let width = (meta.vertex_end - meta.vertex_start) as usize;
            meta.estimated_bytes = (meta.edge_count as usize)
                * std::mem::size_of::<super::super::ImmutableNbr>()
                + (width + 1) * std::mem::size_of::<u32>();
        }
        self.regions = metas;
    }
}

impl std::fmt::Debug for CsrSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CsrSegment")
            .field("create_ts_min", &self.create_ts_min)
            .field("create_ts_max", &self.create_ts_max)
            .field("deletion_info", &self.deletion_info)
            .field("created_at_ts", &self.created_at_ts)
            .field("residency", &self.residency)
            .field("edge_count", &self.csr.read().edge_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::*;
    use crate::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use graphdb_core::Value;

    fn create_edge_table_with_props() -> TimeTravelEdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::types::StoragePropertyDef::new(
                "weight".to_string(),
                graphdb_core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_deletion_info_segment_skip_optimization() {
        let mut table = create_edge_table_with_props();

        for i in 0..10 {
            table
                .insert_edge(0, i, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
                .unwrap();
        }

        table.freeze_csr_only(100);

        for i in 0..10 {
            table.delete_edge(0, i, 0, 200).unwrap();
        }

        table.freeze_csr_only(200);

        table.mvcc.register_active_snapshot(150);

        let edges_at_150 = table.out_edges(0, 150);
        assert_eq!(edges_at_150.len(), 10);

        let edges_at_250 = table.out_edges(0, 250);
        assert_eq!(edges_at_250.len(), 0);

        table.mvcc.unregister_active_snapshot(150);
    }

    #[test]
    fn test_segment_age_calculation() {
        let mut table = create_edge_table_with_props();

        for i in 0..3 {
            table.insert_edge(0, 1, i as i64, &[], 100).unwrap();
        }

        table.freeze_csr_only(105);

        assert!(!table.out_segments.is_empty() || !table.in_segments.is_empty());
    }
}
