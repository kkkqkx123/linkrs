use graphdb_core::types::Timestamp;
use graphdb_core::Value;

#[derive(Debug, Clone)]
pub struct EdgeTableConfig {
    pub initial_vertex_capacity: usize,
    pub initial_edge_capacity: usize,
    /// Fixed number of edges allocated per high-degree overflow chunk.
    pub overflow_chunk_edges: usize,
    /// Write backpressure: max size of the single-segment CSR (in bytes)
    /// before background compaction is requested. Set to 0 to disable.
    pub max_mutable_csr_bytes: usize,
    /// Automatic maintenance: tombstone GC and property compaction on the
    /// write path when the configured thresholds are exceeded.
    pub auto_maintenance: AutoMaintenanceConfig,
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
    /// Write-path calls between automatic maintenance attempts while the GC
    /// watermark is pinned. The counter advances on every write-path call so
    /// the cadence cannot stick: a watermark advance always attempts
    /// immediately, otherwise at most one attempt per this many calls.
    /// Set to 0 to only attempt on watermark advances.
    pub gc_min_serial: u64,
}

impl Default for AutoMaintenanceConfig {
    fn default() -> Self {
        Self {
            tombstone_gc_threshold: 200_000,
            property_compact_ratio: 0.15,
            gc_min_serial: 500,
        }
    }
}

impl Default for EdgeTableConfig {
    fn default() -> Self {
        Self {
            initial_vertex_capacity: 4096,
            initial_edge_capacity: 4096,
            overflow_chunk_edges: 4096,
            max_mutable_csr_bytes: 100 * 1024 * 1024,
            auto_maintenance: AutoMaintenanceConfig::default(),
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
