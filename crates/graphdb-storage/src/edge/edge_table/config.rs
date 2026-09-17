use graphdb_core::types::Timestamp;
use graphdb_core::Value;

#[derive(Debug, Clone)]
pub struct EdgeTableConfig {
    pub initial_vertex_capacity: usize,
    /// Fixed number of edges allocated per high-degree overflow chunk.
    pub overflow_chunk_edges: usize,
    /// Address bits per topology node group: one group covers
    /// `1 << node_group_bits` bound-endpoint rows. Out groups partition by
    /// source, in groups by destination.
    pub node_group_bits: u32,
    /// Write backpressure: max size of the node-group CSR (in bytes)
    /// before background compaction is requested. Set to 0 to disable.
    pub max_mutable_csr_bytes: usize,
    /// Automatic maintenance: property compaction on the
    /// write path when the configured thresholds are exceeded.
    pub auto_maintenance: AutoMaintenanceConfig,
}

/// Thresholds that trigger automatic maintenance on the write path.
#[derive(Debug, Clone, Copy)]
pub struct AutoMaintenanceConfig {
    /// Run property compaction when deleted-but-not-reclaimed property rows
    /// exceed this ratio of total rows. Set to 0.0 to disable.
    pub property_compact_ratio: f32,
}

impl Default for AutoMaintenanceConfig {
    fn default() -> Self {
        Self {
            property_compact_ratio: 0.15,
        }
    }
}

impl Default for EdgeTableConfig {
    fn default() -> Self {
        Self {
            initial_vertex_capacity: 4096,
            overflow_chunk_edges: 4096,
            node_group_bits: crate::edge::node_group::DEFAULT_NODE_GROUP_BITS,
            max_mutable_csr_bytes: 100 * 1024 * 1024,
            auto_maintenance: AutoMaintenanceConfig::default(),
        }
    }
}

/// Parameters for keyed edge property update
pub struct UpdateEdgePropertyByKeyParams {
    pub src: u32,
    pub dst: u32,
    pub rank: i64,
    pub prop_id: u16,
    pub value: Value,
    pub ts: Timestamp,
}
