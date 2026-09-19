use graphdb_core::types::Timestamp;
use graphdb_core::Value;

use crate::edge::RecordFormPreference;

#[derive(Debug, Clone)]
pub struct EdgeTableConfig {
    pub initial_vertex_capacity: usize,
    /// Fixed number of edges allocated per high-degree overflow chunk.
    pub overflow_chunk_edges: usize,
    /// Address bits per topology node group: one group covers
    /// `1 << node_group_bits` bound-endpoint rows. Out groups partition by
    /// source, in groups by destination.
    ///
    /// Locked at table creation: there is no online width-change branch.
    /// The only adjustment outlet is the offline `EdgeStore::reshard` tool,
    /// which rebuilds the table group by group and switches the manifest on
    /// the next checkpoint. Endpoints are expected dense; sparse large
    /// endpoints must be densified offline via vertex remapping first,
    /// otherwise only existing groups consume memory and files while the
    /// span stays wide.
    pub node_group_bits: u32,
    /// Write backpressure: max size of the node-group CSR (in bytes)
    /// before background compaction is requested. Set to 0 to disable.
    pub max_mutable_csr_bytes: usize,
    /// Bound for one group append sidecar: disk plus memory op count at or
    /// above this limit forces a base rewrite on the next checkpoint. Set to
    /// 0 to disable, leaving sidecar growth unbounded.
    pub max_append_ops_per_group: usize,
    /// Region density at or above which a dirty region merges as a whole.
    /// Below it only rows holding reclaimable entries are visited. Tune
    /// against checkpoint flushed bytes per live edge; raising it narrows
    /// merges and never changes the live set.
    pub region_merge_min_density: f32,
    /// Group density at or above which a multi-region dirty span merges at
    /// group scope. Below it merges stay region-scoped. Same tuning source
    /// and safety as the region threshold.
    pub group_merge_min_density: f32,
    /// Automatic maintenance: property compaction on the
    /// write path when the configured thresholds are exceeded.
    pub auto_maintenance: AutoMaintenanceConfig,
    /// Checkpoint topology dump mode: when true, multi-edge groups persist
    /// topology columns at native widths (raw direct dump) instead of the
    /// integer column encoding. Speed-sensitive checkpoints only; files grow
    /// since no bit-packing or run-length encoding applies. Safe to flip
    /// between checkpoints: the dump version marker records the mode per
    /// group file and loads dispatch by marker.
    pub csr_dump_raw: bool,
    /// User-facing preference for record form selection at table creation.
    /// `Auto` lets the system pick Pure/Bundled/Columnar based on schema;
    /// `Columnar` forces the standard multi/single/none strategy path.
    pub record_form: RecordFormPreference,
}

/// Thresholds that trigger automatic maintenance on the write path.
#[derive(Debug, Clone, Copy)]
pub struct AutoMaintenanceConfig {
    /// Run property compaction when deleted-but-not-reclaimed property rows
    /// exceed this ratio of total rows. Set to 0.0 to disable.
    pub property_compact_ratio: f32,
    /// Minimum tracked tombstones arming the write-path reclaim pass when the
    /// watermark did not advance. Below this count with an unchanged watermark
    /// the commit skips the group scan entirely.
    pub reclaim_tombstone_threshold: usize,
}

impl Default for AutoMaintenanceConfig {
    fn default() -> Self {
        Self {
            property_compact_ratio: 0.15,
            reclaim_tombstone_threshold: 4,
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
            max_append_ops_per_group: 4096,
            region_merge_min_density: crate::edge::node_group::REGION_MERGE_MIN_DENSITY,
            group_merge_min_density: crate::edge::node_group::GROUP_MERGE_MIN_DENSITY,
            auto_maintenance: AutoMaintenanceConfig::default(),
            csr_dump_raw: false,
            record_form: RecordFormPreference::default(),
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
