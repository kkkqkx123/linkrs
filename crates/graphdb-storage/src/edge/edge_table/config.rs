use super::calibrator::CalibratorConfig;
use graphdb_core::types::Timestamp;
use graphdb_core::Value;

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

    /// Calibrator configuration for density balancing.
    pub calibrator: CalibratorConfig,

    /// Maximum number of regions frozen per incremental freeze operation.
    /// `0` means unlimited (full freeze, legacy behavior). With `N > 0`,
    /// each freeze incrementally freezes at most `N` high-density regions,
    /// leaving low-density regions in the mutable CSR to reduce per-freeze
    /// latency. Default 8 balances latency and progress.
    pub max_regions_per_freeze: usize,

    /// Density threshold for incremental freeze: a region is considered high-density
    /// when `edge_count / capacity >= threshold`. Calibrator may lower this threshold
    /// under memory pressure. Default 0.05 (5%).
    pub freeze_density_threshold: f32,
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
            max_mutable_csr_bytes: 100 * 1024 * 1024,
            segment_merge_threshold: 50,
            merge_keep_newest: 5,
            auto_maintenance: AutoMaintenanceConfig::default(),
            region_vertex_count: super::segment::DEFAULT_REGION_VERTEX_COUNT,
            version_chain_cap: crate::edge::property_schema::DEFAULT_VERSION_CHAIN_CAP,
            calibrator: CalibratorConfig::default(),
            max_regions_per_freeze: 8,
            freeze_density_threshold: 0.05,
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
