//! Core EdgeStore operations: CRUD, properties, queries, and compaction.
//!
//! Node-group sharded edge table: one sharded CSR per direction plus
//! centralized row-level timestamps. There are no frozen segments, no
//! merges, and no cross-segment deduplication.

use super::super::{CsrBase, CsrShardSet, EdgeRecord, EdgeSchema, MutableCsrTrait, Nbr};
use super::mvcc::MVCCManager;
use super::schema_add_column::PendingAddColumn;
use super::schema_drop_column::PendingDropColumn;
use super::staging::EdgeStagingBatch;
use super::stats::GroupSegmentStats;
use crate::cursor::ScanPredicate;
use crate::edge::property_schema::PropertySchema;
use crate::edge::{CsrWithProperties, VertexFragmentation};
use crate::index::edge_index_manager::EdgePropertyIndex;
use crate::schema::{ChangeDetails, LabelVersionHistory, PropertyChange, SchemaObjectType};
use crate::types::PropertyId;
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{DataType, StorageError, StorageResult, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub use super::config::{AutoMaintenanceConfig, EdgeTableConfig, UpdateEdgePropertyByKeyParams};
pub use super::iterator::EdgeTableScanIterator;

/// Node-group sharded edge store: one sharded CSR per direction with MVCC
/// row timestamps.
pub struct EdgeStore {
    pub label: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub schema: EdgeSchema,
    pub out_csr: CsrShardSet,
    pub in_csr: CsrShardSet,
    pub mvcc: MVCCManager,
    pub properties: CsrWithProperties,
    /// Whether property columns changed since the last checkpoint.
    pub properties_dirty: bool,
    pub is_open: bool,
    pub next_edge_id: EdgeId,
    pub config: EdgeTableConfig,
    pub stats_manager: Option<std::sync::Arc<graphdb_metrics::StatsManager>>,
    /// Version history tracking for schema changes
    pub version_history: Arc<Mutex<LabelVersionHistory>>,
    /// Cache for property name → schema index mapping to avoid O(n) linear lookups.
    /// Invalidated whenever schema changes.
    pub property_index_cache: HashMap<String, usize>,

    /// Edge property index for efficient property-based filtering.
    ///
    /// Best-effort asynchronous secondary structure, not a synchronous part
    /// of the write: insert/delete failures are counted in
    /// `index_write_failures` and reported to the metrics registry, never
    /// failing the primary write. Primary data stays authoritative while the
    /// index may lag; operators rebuild via `build_property_index` (or
    /// `rebuild_property_index_on_failures`) once failures cross a chosen
    /// threshold. A full streaming rebuild resets the lag baseline.
    pub property_index: Option<EdgePropertyIndex>,
    /// Secondary index write failures since the last rebuild or reset.
    /// Observability only; primary data stays authoritative.
    pub index_write_failures: u64,

    /// In-flight staged add-column change. Memory-only: a crash before
    /// publishing is equivalent to aborting, because reload rebuilds the
    /// property store from the published schema.
    pub(crate) pending_add_column: Option<PendingAddColumn>,
    /// In-flight staged drop-column change. Memory-only, same crash contract
    /// as the staged add: at most one schema change is pending at a time.
    pub(crate) pending_drop_column: Option<PendingDropColumn>,
    /// Owner group for timestamp and property sharding, keyed by edge id.
    /// The owner is the out group when out edges exist, otherwise the in
    /// group. Shard files follow the owner groups with the same dirt, so
    /// small writes rewrite only dirty owners. Rebuilt on load, remap and
    /// reshard; orphan timestamps without topology fall back to group zero.
    pub(crate) edge_owner: HashMap<EdgeId, u32>,
    /// Per-group segment statistics for scan pruning, collected at each
    /// checkpoint and restored on load. Bounds widen monotonically so pruning
    /// stays conservative for every snapshot; counts are exact-current for
    /// observability.
    pub(crate) segment_stats: HashMap<u32, GroupSegmentStats>,
    /// Reusable commit working buffers, cleared and recycled every commit so
    /// small batches pay no per-commit allocation for bookkeeping.
    pub(crate) commit_scratch: super::staging::CommitScratch,
    /// Watermark bound of the last executed reclaim pass. The write-path
    /// reclaim pass is skipped while the bound is unchanged and the tombstone
    /// heap stays below threshold, so insert-heavy commits pay no scan.
    pub(crate) last_reclaim_bound: Timestamp,
    /// Tombstone count seen by the last executed reclaim pass. Growth past
    /// this baseline re-arms the pass even when the watermark stands still.
    pub(crate) last_reclaim_tombstones: usize,
    /// Directory of the last checkpoint or load, owning the write-ahead log.
    /// Commits append logical redo here before returning success; checkpoints
    /// truncate it after the new snapshot is durable. `None` before the first
    /// checkpoint, when redo has no home yet.
    pub(crate) wal_dir: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for EdgeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeStore")
            .field("label", &self.label)
            .field("label_name", &self.label_name)
            .field("out_csr", &self.out_csr)
            .field("in_csr", &self.in_csr)
            .field("is_open", &self.is_open)
            .field("next_edge_id", &self.next_edge_id)
            .field("config", &self.config)
            .finish()
    }
}

impl EdgeStore {
    pub fn new(schema: EdgeSchema) -> StorageResult<Self> {
        Self::with_config(schema, EdgeTableConfig::default())
    }

    pub fn with_config(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        schema.validate()?;

        if config.overflow_chunk_edges == 0 {
            return Err(StorageError::invalid_operation(
                "overflow_chunk_edges must be greater than zero",
            ));
        }

        let mut out_csr = CsrShardSet::new(
            schema.oe_strategy,
            config.node_group_bits,
            config.overflow_chunk_edges,
        )?;
        let mut in_csr = CsrShardSet::new(
            schema.ie_strategy,
            config.node_group_bits,
            config.overflow_chunk_edges,
        )?;
        // Pre-create groups covering the configured initial row space so
        // small tables start with their full address range addressable.
        let initial_groups = config
            .initial_vertex_capacity
            .div_ceil(out_csr.group_size())
            .max(1);
        if schema.oe_strategy != super::super::EdgeStrategy::None {
            out_csr.resize_groups(initial_groups)?;
        }
        if schema.ie_strategy != super::super::EdgeStrategy::None {
            in_csr.resize_groups(initial_groups)?;
        }

        let prop_schemas: Vec<PropertySchema> = schema
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| {
                PropertySchema::new(p.name.clone(), i as i32, p.data_type.clone())
                    .nullable(p.nullable)
                    .with_default_value(p.default_value.clone())
            })
            .collect();
        let properties = CsrWithProperties::new(prop_schemas);

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
            mvcc: MVCCManager::new(),
            properties,
            properties_dirty: false,
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            version_history,
            property_index_cache,
            property_index: None,
            index_write_failures: 0,
            pending_add_column: None,
            pending_drop_column: None,
            edge_owner: HashMap::new(),
            segment_stats: HashMap::new(),
            commit_scratch: super::staging::CommitScratch::default(),
            last_reclaim_bound: Timestamp::MAX,
            last_reclaim_tombstones: 0,
            wal_dir: None,
        })
    }

    /// Owner group for one edge write. Out groups own when out edges exist,
    /// otherwise in groups own. The owner decides which timestamp and
    /// property shard carries the edge with the same dirt as its topology.
    pub(crate) fn owner_gid_for(&self, src: u32, dst: u32) -> u32 {
        if self.schema.oe_strategy != super::super::EdgeStrategy::None {
            crate::edge::node_group::group_id_for(src, self.config.node_group_bits) as u32
        } else {
            crate::edge::node_group::group_id_for(dst, self.config.node_group_bits) as u32
        }
    }

    /// Existing owner groups in group order. Timestamp and property shards
    /// follow exactly these groups; missing groups have no shard files.
    pub(crate) fn owner_group_ids(&self) -> Vec<u32> {
        let owner = if self.schema.oe_strategy != super::super::EdgeStrategy::None {
            &self.out_csr
        } else {
            &self.in_csr
        };
        owner
            .existing_group_ids()
            .into_iter()
            .map(|gid| gid as u32)
            .collect()
    }

    /// Rebuild the owner map from topology plus authority leftovers.
    /// Topology edges take their current owner; timestamps or property rows
    /// without topology (reclaimed physical rows whose authority tombstone
    /// survives) fall back to the smallest materialized owner so they stay
    /// in an existing shard.
    pub(crate) fn rebuild_owner_map(&mut self) {
        self.edge_owner.clear();
        let use_out = self.schema.oe_strategy != super::super::EdgeStrategy::None;
        let owner = if use_out { &self.out_csr } else { &self.in_csr };
        let existing = owner.existing_group_ids();
        for gid in &existing {
            if let Some(variant) = owner.group_variant(*gid) {
                for (_, nbr) in variant.iter_all() {
                    self.edge_owner.insert(nbr.edge_id, *gid as u32);
                }
            }
        }
        let fallback = existing.first().copied().unwrap_or(0) as u32;
        for edge_id in self.mvcc.edge_timestamps.keys() {
            self.edge_owner.entry(*edge_id).or_insert(fallback);
        }
        for edge_id in self.properties.edge_ids() {
            self.edge_owner.entry(edge_id).or_insert(fallback);
        }
    }

    pub(crate) fn edge_endpoint_key(endpoint: u32, rank: i64) -> VertexId {
        VertexId::edge_endpoint_key(endpoint, rank)
    }

    pub(crate) fn resolve_owner_gid(
        edge_id: &EdgeId,
        edge_owner: &HashMap<EdgeId, u32>,
        live: &HashSet<u32>,
        fallback: Option<u32>,
    ) -> (u32, bool) {
        let owner = edge_owner.get(edge_id).copied().unwrap_or(0);
        if live.contains(&owner) {
            (owner, false)
        } else {
            (fallback.unwrap_or(0), true)
        }
    }

    pub(crate) fn decode_edge_endpoint(key: VertexId) -> (VertexId, i64) {
        let bytes = key.as_bytes();
        if bytes.len() != 16 {
            log::warn!(
                "decode_edge_endpoint: unexpected key length {}, expected 16",
                bytes.len()
            );
        }
        key.decode_edge_endpoint()
    }

    pub fn set_stats_manager(&mut self, stats: std::sync::Arc<graphdb_metrics::StatsManager>) {
        self.stats_manager = Some(stats);
    }

    /// Secondary index write failures since the last rebuild or reset.
    pub fn index_failure_count(&self) -> u64 {
        self.index_write_failures
    }

    /// Reset the secondary index failure counter, typically after a rebuild.
    pub fn reset_index_failures(&mut self) {
        self.index_write_failures = 0;
    }

    /// Rebuild the property index when failures cross `threshold`.
    ///
    /// Returns true when a rebuild ran. Threshold policy lives with the
    /// operator; the store only guarantees the counter is monotonic between
    /// resets. A rebuild scans all live edges, so the fresh index resets the
    /// lag baseline to the build's own failure count.
    pub fn rebuild_property_index_on_failures(
        &mut self,
        threshold: u64,
        pool_capacity: u64,
    ) -> StorageResult<bool> {
        if self.property_index.is_none() || self.index_write_failures < threshold {
            return Ok(false);
        }
        self.build_property_index(pool_capacity)?;
        Ok(true)
    }

    fn record_index_write_failure(&mut self, prop_name: &str, latency_ms: u64) {
        self.index_write_failures = self.index_write_failures.saturating_add(1);
        if let Some(stats) = &self.stats_manager {
            stats.record_index_operation(self.label as u64, prop_name, latency_ms, false);
        }
    }

    fn record_index_write_success(&self, prop_name: &str, latency_ms: u64) {
        if let Some(stats) = &self.stats_manager {
            stats.record_index_operation(self.label as u64, prop_name, latency_ms, true);
        }
    }

    /// Fold one secondary index write outcome into the lag counter and the
    /// shared metrics registry. Primary data stays authoritative regardless.
    fn note_index_result(
        &mut self,
        prop_name: &str,
        result: StorageResult<()>,
        latency_ms: u64,
    ) {
        match result {
            Ok(()) => self.record_index_write_success(prop_name, latency_ms),
            Err(_) => self.record_index_write_failure(prop_name, latency_ms),
        }
    }

    /// Shared physical row location for point lookups: one physical topology
    /// address plus the authoritative MVCC check. Adjacency batches, full
    /// scans and point lookups all resolve rows through this routing instead
    /// of duplicating group arithmetic; scans additionally share
    /// [`EdgeStore::is_visible`] as the single visibility gate.
    pub(crate) fn physical_location(
        &self,
        csr: &CsrShardSet,
        src: u32,
        dst: VertexId,
    ) -> Option<Nbr> {
        csr.get_edge_physical(src, dst)
    }

    /// Single visibility gate for topology reads. Row stamps never decide
    /// visibility; only the version authority does.
    pub(crate) fn is_visible(&self, edge_id: EdgeId, ts: Timestamp) -> bool {
        self.mvcc.is_edge_visible(edge_id, ts)
    }

    /// Pending-aware variant of the shared visibility gate.
    pub(crate) fn is_visible_with_gate(
        &self,
        edge_id: EdgeId,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> bool {
        self.mvcc.is_edge_visible_with_gate(edge_id, ts, gate)
    }

    /// Fill a caller buffer with every visible neighbor of one row without
    /// internal allocation. Shared batch primitive behind the adjacency
    /// accessor and the allocating convenience wrappers below.
    pub(crate) fn fill_visible_into(
        &self,
        csr: &CsrShardSet,
        src: u32,
        ts: Timestamp,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        csr.visit_physical(src, |nbr| {
            if self.is_visible(nbr.edge_id, ts) {
                out.push(nbr);
            }
            true
        });
    }

    /// Pending-aware variant of the shared batch fill.
    pub(crate) fn fill_visible_into_with_gate(
        &self,
        csr: &CsrShardSet,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        csr.visit_physical(src, |nbr| {
            if self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                out.push(nbr);
            }
            true
        });
    }

    /// Single row-location entry for point lookups: physical topology lookup
    /// plus the authoritative MVCC visibility check. Adjacency, existence
    /// and record reads must funnel through here rather than reading CSR
    /// timestamps directly; row stamps exist only for collection. Scans all
    /// physical matches for the key so a delete-then-rebuild pair (old
    /// tombstone plus new live row sharing one endpoint key) resolves to the
    /// visible live row instead of the first physical slot.
    fn merged_get_edge(
        &self,
        csr: &CsrShardSet,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
    ) -> Option<Nbr> {
        let mut found = None;
        csr.visit_physical(src, |nbr| {
            if nbr.to_vertex_id() == dst && self.is_visible(nbr.edge_id, ts) {
                found = Some(nbr);
                false
            } else {
                true
            }
        });
        found
    }

    /// Allocating convenience over the shared batch fill. High-frequency
    /// traversal and batch queries use the reusable-buffer accessor instead;
    /// this wrapper stays for call sites where an owned vector is handier.
    fn merged_edges_of(&self, csr: &CsrShardSet, src: u32, ts: Timestamp) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_visible_into(csr, src, ts, &mut out);
        out
    }

    /// Allocating pending-aware convenience over the shared batch fill.
    fn merged_edges_of_with_gate(
        &self,
        csr: &CsrShardSet,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_visible_into_with_gate(csr, src, ts, gate, &mut out);
        out
    }

    /// First-`limit` visible neighbors without decoding the full adjacency.
    /// Uses the physical visit path so high-degree `LIMIT` queries stop
    /// after `k` visible neighbors instead of decoding every edge plus properties.
    pub fn merged_out_nbrs_with_limit(&self, src: u32, ts: Timestamp, limit: usize) -> Vec<Nbr> {
        self.merged_nbrs_with_limit(&self.out_csr, src, ts, limit)
    }

    /// First-`limit` visible in-neighbors, mirroring the out direction.
    pub fn merged_in_nbrs_with_limit(&self, dst: u32, ts: Timestamp, limit: usize) -> Vec<Nbr> {
        self.merged_nbrs_with_limit(&self.in_csr, dst, ts, limit)
    }

    fn merged_nbrs_with_limit(
        &self,
        csr: &CsrShardSet,
        vid: u32,
        ts: Timestamp,
        limit: usize,
    ) -> Vec<Nbr> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit.min(32));
        csr.visit_physical(vid, |nbr| {
            if self.is_visible(nbr.edge_id, ts) {
                out.push(nbr);
                out.len() < limit
            } else {
                true
            }
        });
        out
    }

    pub fn merged_out_nbrs_with_gate(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        self.merged_edges_of_with_gate(&self.out_csr, src, ts, gate)
    }

    pub fn merged_in_nbrs_with_gate(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        self.merged_edges_of_with_gate(&self.in_csr, dst, ts, gate)
    }

    pub fn merged_out_nbrs_with_gate_limit(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        limit: usize,
    ) -> Vec<Nbr> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit.min(32));
        self.out_csr.visit_physical(src, |nbr| {
            if self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                out.push(nbr);
                out.len() < limit
            } else {
                true
            }
        });
        out
    }

    pub fn merged_in_nbrs_with_gate_limit(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        limit: usize,
    ) -> Vec<Nbr> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit.min(32));
        self.in_csr.visit_physical(dst, |nbr| {
            if self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                out.push(nbr);
                out.len() < limit
            } else {
                true
            }
        });
        out
    }

    pub fn out_edges_with_gate(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        self.out_edges_with_gate_projected(src, ts, gate, None)
    }

    pub fn out_edges_with_gate_projected(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        self.merged_out_nbrs_with_gate(src, ts, gate)
            .into_iter()
            .map(|nbr| {
                let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid,
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub fn in_edges_with_gate(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        self.in_edges_with_gate_projected(dst, ts, gate, None)
    }

    pub fn in_edges_with_gate_projected(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        self.merged_in_nbrs_with_gate(dst, ts, gate)
            .into_iter()
            .map(|nbr| {
                let src_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub fn out_edges_with_gate_projected_limit(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
        limit: usize,
    ) -> Vec<EdgeRecord> {
        if !self.is_open || limit == 0 {
            return Vec::new();
        }
        self.merged_out_nbrs_with_gate_limit(src, ts, gate, limit)
            .into_iter()
            .map(|nbr| {
                let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid,
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub fn in_edges_with_gate_projected_limit(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
        limit: usize,
    ) -> Vec<EdgeRecord> {
        if !self.is_open || limit == 0 {
            return Vec::new();
        }
        self.merged_in_nbrs_with_gate_limit(dst, ts, gate, limit)
            .into_iter()
            .map(|nbr| {
                let src_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub(crate) fn edge_record_from_nbr(
        &self,
        src: u32,
        nbr: Nbr,
        query_ts: Timestamp,
    ) -> EdgeRecord {
        self.edge_record_from_nbr_projected(src, nbr, query_ts, None)
    }

    pub(crate) fn edge_record_from_nbr_projected(
        &self,
        src: u32,
        nbr: Nbr,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> EdgeRecord {
        let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
        let rank = nbr.rank;
        let properties = self.properties_for_edge_projected(nbr.edge_id, query_ts, projection);
        EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid,
            rank,
            properties,
        }
    }

    fn properties_for_edge(&self, edge_id: EdgeId, query_ts: Timestamp) -> Vec<(String, Value)> {
        self.properties_for_edge_projected(edge_id, query_ts, None)
    }

    /// Topology-first property read: MVCC authority decides visibility,
    /// then only the projected columns are decoded. `None` decodes all
    /// columns, `Some(&[])` decodes none. Null-valued columns are filtered
    /// out, so callers cannot distinguish NULL from a missing column; the
    /// streaming cursor path shares this projection contract.
    fn properties_for_edge_projected(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
        // MVCCManager is the single visibility authority. Row stamps exist
        // only for collection and must not decide query visibility here.
        if !self.is_visible(edge_id, query_ts) {
            return Vec::new();
        }
        // Snapshot read through the property version chain so an old reader
        // observes the before-image, not the latest write.
        self.properties
            .get_projected_physical_by_edge_id(edge_id, query_ts, projection)
            .map(|rows| {
                rows.into_iter()
                    .filter_map(|(name, value)| value.map(|v| (name, v)))
                    .collect()
            })
            .unwrap_or_default()
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

        // Single-entry staging commit: the batch owns the whole multi-step
        // write, so failure handling lives in one place below instead of in
        // per-step compensation branches here.
        let mut batch = EdgeStagingBatch::new();
        batch.stage_insert(src, dst, rank, property_values, ts);
        self.commit_staging_batch(batch).map(|_| ())
    }

    /// Create an empty staging batch for one atomic group of edge writes.
    pub fn staging_batch() -> EdgeStagingBatch {
        EdgeStagingBatch::new()
    }

    /// Commit one staging batch atomically.
    ///
    /// Entries apply in stage order with batch prefix effects: a later entry
    /// observes earlier entries of the same batch. An insert cancelled by a
    /// later delete of the same key leaves no tombstone and no visible edge;
    /// a delete followed by an insert of the same key rebuilds. Staged writes
    /// stay invisible to every read until commit. Prevalidation failures leave
    /// committed state untouched. When a late failure still occurs, only the
    /// entries this batch already applied are rolled back; committed data from
    /// other batches is never touched. Dropping a batch without committing
    /// discards it with no residue. Cancelled inserts consume at most the
    /// monotonic edge-id counter, never visible state.
    ///
    /// Returns the number of net applied entries (inserts plus deletes,
    /// excluding batch-cancelled pairs).
    pub fn commit_staging_batch(&mut self, mut batch: EdgeStagingBatch) -> StorageResult<usize> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if batch.insert_count() > 0 && self.schema.oe_strategy == super::super::EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }
        self.prevalidate_staging_batch(&batch)?;
        if let Some(dir) = self.wal_dir.clone() {
            let mut ops = Vec::with_capacity(batch.len());
            for ord in batch.ordered() {
                if ord.is_insert {
                    let ins = &batch.staged_inserts()[ord.slot];
                    ops.push(super::wal::EdgeWalOp::Insert {
                        src: ins.src,
                        dst: ins.dst,
                        rank: ins.rank,
                        properties: ins.properties.clone(),
                        create_ts: ins.create_ts,
                    });
                } else {
                    let del = &batch.staged_deletes()[ord.slot];
                    ops.push(super::wal::EdgeWalOp::Delete {
                        src: del.src,
                        dst: del.dst,
                        rank: del.rank,
                        delete_ts: del.delete_ts,
                    });
                }
            }
            super::wal::append_ops(&dir, &ops)?;
        }
        let max_ts = batch.max_timestamp();
        let inserts = batch.take_inserts();
        let deletes = batch.take_deletes();
        let order = batch.take_order();
        if order.is_empty() {
            return Ok(0);
        }

        // Recycled working buffers: cleared, not reallocated, and handed
        // back on every exit path below so the next commit reuses them.
        let mut scratch = self.commit_scratch.take();
        scratch.reset(inserts.len(), deletes.len());
        let applied_inserts = &mut scratch.applied_inserts;
        let insert_by_key = &mut scratch.insert_by_key;
        let applied_deletes = &mut scratch.applied_deletes;
        for ord in &order {
            if ord.is_insert {
                let ins = &inserts[ord.slot];
                match self.apply_staged_insert(
                    ins.src,
                    ins.dst,
                    ins.rank,
                    &ins.properties,
                    ins.create_ts,
                ) {
                    Ok(edge_id) => {
                        let entry = (ins.src, ins.dst, ins.rank, edge_id, ins.create_ts);
                        applied_inserts.push(entry);
                        insert_by_key.insert((ins.src, ins.dst, ins.rank), entry);
                    }
                    Err(e) => {
                        for (src, dst, _rank, edge_id, ts) in applied_deletes.drain(..) {
                            self.revert_applied_delete(src, dst, edge_id, ts);
                        }
                        for (src, dst, rank, edge_id, ts) in applied_inserts.drain(..) {
                            self.erase_applied_insert(src, dst, rank, edge_id, ts);
                        }
                        self.commit_scratch = scratch;
                        return Err(e);
                    }
                }
            } else {
                let del = &deletes[ord.slot];
                let key = (del.src, del.dst, del.rank);
                if let Some((src, dst, rank, edge_id, ts)) = insert_by_key.remove(&key) {
                    self.erase_applied_insert(src, dst, rank, edge_id, ts);
                    applied_inserts.retain(|(_, _, _, eid, _)| *eid != edge_id);
                    continue;
                }
                match self.apply_staged_delete(del.src, del.dst, del.rank, del.delete_ts) {
                    Ok(Some(edge_id)) => {
                        applied_deletes.push((del.src, del.dst, del.rank, edge_id, del.delete_ts));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        for (src, dst, _rank, edge_id, ts) in applied_deletes.drain(..) {
                            self.revert_applied_delete(src, dst, edge_id, ts);
                        }
                        for (src, dst, rank, edge_id, ts) in applied_inserts.drain(..) {
                            self.erase_applied_insert(src, dst, rank, edge_id, ts);
                        }
                        self.commit_scratch = scratch;
                        return Err(e);
                    }
                }
            }
        }

        let applied = applied_inserts.len() + applied_deletes.len();
        if applied > 0 {
            // Backpressure is observed, not dropped: an over-limit commit
            // warns and the maintenance pass below runs synchronously.
            let mut pressured = false;
            if let Some(ts) = max_ts {
                pressured = self.check_and_apply_write_backpressure(ts);
            }
            self.maybe_run_auto_maintenance();
            if pressured {
                log::warn!(
                    "edge table '{}' over mutable CSR budget, synchronous maintenance ran",
                    self.label_name
                );
            }
            for (_, _, _, edge_id, _) in applied_inserts.iter().chain(applied_deletes.iter()) {
                self.debug_assert_copies_consistent(*edge_id);
            }
        }
        self.commit_scratch = scratch;
        Ok(applied)
    }

    fn convert_property_values(
        &self,
        property_values: &[(String, Value)],
    ) -> StorageResult<Vec<(String, Value)>> {
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
        Ok(converted_values)
    }

    fn prevalidate_staging_batch(&self, batch: &EdgeStagingBatch) -> StorageResult<()> {
        use std::collections::HashSet;
        let mut seen_inserts: HashSet<(u32, u32, i64)> = HashSet::new();
        let mut seen_deletes: HashSet<(u32, u32, i64)> = HashSet::new();
        let mut seen_single_src: HashSet<u32> = HashSet::new();
        let mut seen_single_dst: HashSet<u32> = HashSet::new();
        let single_out = self.schema.oe_strategy == super::super::EdgeStrategy::Single;
        let single_in = self.schema.ie_strategy == super::super::EdgeStrategy::Single;
        for ord in batch.ordered() {
            if ord.is_insert {
                let ins = &batch.staged_inserts()[ord.slot];
                for (name, _) in &ins.properties {
                    if !self.property_index_cache.contains_key(name) {
                        return Err(StorageError::column_not_found(name.clone()));
                    }
                }
                let key = (ins.src, ins.dst, ins.rank);
                if seen_inserts.contains(&key) {
                    return Err(StorageError::edge_already_exists(format!(
                        "{} -> {}@{}",
                        ins.src, ins.dst, ins.rank
                    )));
                }
                if single_out && seen_single_src.contains(&ins.src) {
                    return Err(StorageError::conflict(format!(
                        "Single out-edge strategy already holds a live edge for src={}",
                        ins.src
                    )));
                }
                if single_in && seen_single_dst.contains(&ins.dst) {
                    return Err(StorageError::conflict(format!(
                        "Single in-edge strategy already holds a live edge for dst={}",
                        ins.dst
                    )));
                }
                if single_out {
                    let live = self.merged_edges_of(&self.out_csr, ins.src, ins.create_ts);
                    let covered = !live.is_empty()
                        && live
                            .iter()
                            .all(|nbr| seen_deletes.contains(&(ins.src, nbr.endpoint, nbr.rank)));
                    if !live.is_empty() && !covered {
                        return Err(StorageError::conflict(format!(
                            "Single out-edge strategy already holds a live edge for src={}",
                            ins.src
                        )));
                    }
                }
                if single_in {
                    let live = self.merged_edges_of(&self.in_csr, ins.dst, ins.create_ts);
                    let covered = !live.is_empty()
                        && live
                            .iter()
                            .all(|nbr| seen_deletes.contains(&(nbr.endpoint, ins.dst, nbr.rank)));
                    if !live.is_empty() && !covered {
                        return Err(StorageError::conflict(format!(
                            "Single in-edge strategy already holds a live edge for dst={}",
                            ins.dst
                        )));
                    }
                }
                if !seen_deletes.contains(&key)
                    && self.has_edge(ins.src, ins.dst, ins.rank, ins.create_ts)
                {
                    return Err(StorageError::edge_already_exists(format!(
                        "{} -> {}@{}",
                        ins.src, ins.dst, ins.rank
                    )));
                }
                if seen_deletes.contains(&key) {
                    seen_deletes.remove(&key);
                }
                seen_inserts.insert(key);
                if single_out {
                    seen_single_src.insert(ins.src);
                }
                if single_in {
                    seen_single_dst.insert(ins.dst);
                }
            } else {
                let del = &batch.staged_deletes()[ord.slot];
                let key = (del.src, del.dst, del.rank);
                if seen_inserts.remove(&key) {
                    if single_out && !seen_inserts.iter().any(|(s, _, _)| *s == del.src) {
                        seen_single_src.remove(&del.src);
                    }
                    if single_in && !seen_inserts.iter().any(|(_, d, _)| *d == del.dst) {
                        seen_single_dst.remove(&del.dst);
                    }
                } else {
                    seen_deletes.insert(key);
                }
            }
        }
        Ok(())
    }

    /// Move one staged insert into the committed structures.
    ///
    /// Self-contained: a failure cleans up only this entry, so the batch
    /// rollback above only handles entries that fully applied.
    fn apply_staged_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<EdgeId> {
        let converted_values = self.convert_property_values(property_values)?;
        let edge_id = self.next_edge_id.fetch_add();

        // No existence re-check here by design. The batch prevalidation is
        // the single main existence check: it already rejected duplicate keys
        // and occupied Single slots for this batch. The topology insert
        // below is the light recheck: its live-set/slot guard rejects the
        // same conflicts in O(1) without another row scan, and the failure
        // path underneath cleans up the record staged above.

        self.mvcc.record_creation(edge_id, ts);

        if let Err(e) = self
            .properties
            .insert_for_edge(edge_id, &converted_values, ts)
        {
            self.mvcc.remove_edge_timestamps(edge_id);
            return Err(e);
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        if let Err(e) = self.out_csr.insert_edge(src, dst_key, edge_id, ts) {
            if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                self.properties.release_row(row);
            }
            self.mvcc.remove_edge_timestamps(edge_id);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if let Err(e) = self.in_csr.insert_edge(dst, src_key, edge_id, ts) {
            // Roll back the out-direction insertion physically so no
            // tombstone residue remains; fall back to logical deletion if
            // the entry cannot be located.
            if !self.out_csr.remove_edge(src, edge_id) {
                let _ = self.out_csr.delete_edge(src, edge_id, ts);
            }
            if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                self.properties.release_row(row);
            }
            let _ = self.properties.mark_deleted(edge_id, ts);
            self.mvcc.remove_edge_timestamps(edge_id);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if self.property_index.is_some() {
            let label = self.label;
            let outcomes: Vec<(String, StorageResult<()>, u64)> = if let Some(ref mut index) =
                self.property_index
            {
                converted_values
                    .iter()
                    .map(|(prop_name, prop_value)| {
                        let started = std::time::Instant::now();
                        let result = index.insert(prop_name, prop_value, src, dst, rank, label, ts);
                        let latency = started.elapsed().as_millis() as u64;
                        (prop_name.clone(), result, latency)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }

        self.mark_properties_dirty();
        self.edge_owner
            .insert(edge_id, self.owner_gid_for(src, dst));
        self.debug_assert_copies_consistent(edge_id);
        Ok(edge_id)
    }

    /// Move one staged delete into the committed structures.
    ///
    /// Full-match endpoint semantics: one call deletes every live match for
    /// the endpoint key and reports the deleted count for rollback
    /// reconciliation. Returns the deleted edge id, or `None` when no edge
    /// matched.
    fn apply_staged_delete(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> StorageResult<Option<EdgeId>> {
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);

        let edge_properties = if self.property_index.is_some() {
            self.get_edge(src, dst, rank, ts).map(|e| e.properties)
        } else {
            None
        };

        if let Some(nbr) = self.merged_get_edge(&self.out_csr, src, dst_key, ts) {
            let edge_id = nbr.edge_id;

            if !self.out_csr.delete_edge(src, edge_id, ts)? {
                return Ok(None);
            }
            let in_deleted = self.in_csr.delete_edge_by_dst(dst, src_key, ts);
            if in_deleted == 0 {
                // Roll back the out-direction deletion to keep both sides
                // consistent. Count reconciliation: expected exactly one
                // in-direction match for the out edge just deleted.
                if !self.out_csr.revert_delete_by_edge_id(src, edge_id, ts) {
                    return Err(StorageError::invalid_operation(format!(
                        "delete rollback failed for edge {:?}: out-direction revert missed",
                        edge_id
                    )));
                }
                return Ok(None);
            }
            if in_deleted > 1 {
                log::debug!(
                    "apply_staged_delete multi-match: ({}, {}, {}) in_deleted={}",
                    src,
                    dst,
                    rank,
                    in_deleted
                );
            }

            self.mvcc.record_edge_deletion(edge_id, ts);
            let _ = self.properties.mark_deleted(edge_id, ts);
            self.update_property_index_on_delete(&edge_properties, src, dst, rank, ts);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Ok(Some(edge_id));
        }

        // Missed the merged read: distinguish absent edges from conflicting
        // re-deletes. Scan every physical generation sharing the endpoint
        // key and check each against the authority, so a stale tombstone
        // generation never masks the visible generation.
        let mut candidates: Vec<EdgeId> = Vec::new();
        self.out_csr.visit_physical(src, |nbr| {
            if nbr.to_vertex_id() == dst_key {
                candidates.push(nbr.edge_id);
            }
            true
        });
        for candidate in candidates {
            if let Some(info) = self.mvcc.edge_timestamps.get(&candidate) {
                if info.delete_ts != Timestamp::MAX && info.delete_ts != ts {
                    return Err(StorageError::write_write_conflict(format!(
                        "edge {:?} already deleted at ts={}, attempted delete at ts={}",
                        candidate, info.delete_ts, ts
                    )));
                }
            }
        }

        Ok(None)
    }

    /// Erase one batch-applied insert during batch rollback.
    ///
    /// Physical removal across all copies; tolerates absence so rollback
    /// stays total even under partial application.
    fn erase_applied_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        edge_id: EdgeId,
        ts: Timestamp,
    ) {
        let properties = self
            .properties
            .read_properties_by_edge_id(edge_id)
            .unwrap_or_default();
        self.out_csr.remove_edge(src, edge_id);
        self.in_csr.remove_edge(dst, edge_id);
        if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
            self.properties.release_row(row);
        }
        self.mvcc.remove_edge_timestamps(edge_id);
        self.edge_owner.remove(&edge_id);
        if self.property_index.is_some() {
            let outcomes: Vec<(String, StorageResult<()>, u64)> =
                if let Some(ref mut index) = self.property_index {
                    properties
                        .iter()
                        .map(|(prop_name, prop_value)| {
                            let started = std::time::Instant::now();
                            let result = index.delete(prop_name, prop_value, src, dst, rank, ts);
                            let latency = started.elapsed().as_millis() as u64;
                            (prop_name.clone(), result, latency)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
    }

    /// Shared gate for authority revival on delete rollback.
    ///
    /// Authority may only return to live when both directions physically
    /// reverted. A partial revert keeps the authority deletion mark so a
    /// future timestamp tombstone can never coexist with a live authority
    /// record.
    #[inline]
    fn fully_reverted(out_ok: bool, in_ok: bool) -> bool {
        out_ok && in_ok
    }

    /// Revert one batch-applied delete during batch rollback.
    fn revert_applied_delete(&mut self, src: u32, dst: u32, edge_id: EdgeId, ts: Timestamp) {
        let out_ok = self.out_csr.revert_delete_by_edge_id(src, edge_id, ts);
        let in_ok = self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts);
        if !Self::fully_reverted(out_ok, in_ok) {
            return;
        }
        if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&edge_id) {
            ts_info.delete_ts = Timestamp::MAX;
        }
        let _ = self.properties.revert_deletion_for_edge(edge_id);
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
    }

    /// Per-vertex fragmentation view combining both directions.
    ///
    /// Observation only; the collection trigger consults `vertex_census`
    /// and `reclaimable_count` directly.
    pub fn vertex_fragmentation(&self, vid: u32, cutoff: Timestamp) -> VertexFragmentation {
        let (out_live, out_dead, out_cap) = self.out_csr.vertex_census(vid);
        let (in_live, in_dead, in_cap) = self.in_csr.vertex_census(vid);
        VertexFragmentation {
            vertex: vid,
            live_edges: out_live + in_live,
            dead_entries: out_dead + in_dead,
            capacity: out_cap + in_cap,
            reclaimable: self.out_csr.reclaimable_count(vid, cutoff)
                + self.in_csr.reclaimable_count(vid, cutoff),
        }
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

        // Single-entry staging commit: the batch owns the two-direction
        // write, so the out/in rollback lives in one place.
        let mut batch = EdgeStagingBatch::new();
        batch.stage_delete(src, dst, rank, ts);
        Ok(self.commit_staging_batch(batch)? > 0)
    }

    /// Physically erase an edge inserted by an uncommitted transaction.
    ///
    /// Insert-undo path: unlike a user delete (logical deletion through
    /// `delete_edge`), aborting an insert must leave no trace in any of the
    /// three copies — otherwise the aborted edge would stay visible inside
    /// its snapshot window and leave a permanent tombstone. Every step
    /// tolerates absence, so replaying the undo (abort re-drive, WAL
    /// recovery re-application) is idempotent.
    pub fn erase_edge(&mut self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        let Some(edge_id) = self.edge_id_of(src, dst, rank, ts) else {
            return false;
        };
        let properties = self
            .properties
            .read_properties_by_edge_id(edge_id)
            .unwrap_or_default();
        self.out_csr.remove_edge(src, edge_id);
        self.in_csr.remove_edge(dst, edge_id);
        if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
            self.properties.release_row(row);
        }
        self.mvcc.remove_edge_timestamps(edge_id);
        self.edge_owner.remove(&edge_id);
        if self.property_index.is_some() {
            let outcomes: Vec<(String, StorageResult<()>, u64)> =
                if let Some(ref mut index) = self.property_index {
                    properties
                        .iter()
                        .map(|(prop_name, prop_value)| {
                            let started = std::time::Instant::now();
                            let result = index.delete(prop_name, prop_value, src, dst, rank, ts);
                            let latency = started.elapsed().as_millis() as u64;
                            (prop_name.clone(), result, latency)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }
        self.debug_assert_copies_consistent(edge_id);
        true
    }

    /// Debug-only cross-copy consistency check for one edge.
    ///
    /// Release builds skip the whole body (zero overhead): every arm is a
    /// `debug_assert`. A property row mapping must never outlive its
    /// authority entry (orphan row). Called on insert success, delete
    /// success and delete-rollback success.
    fn debug_assert_copies_consistent(&self, edge_id: EdgeId) {
        debug_assert!(
            self.properties.get_row_for_edge(edge_id).is_none()
                || self.mvcc.edge_timestamps.contains_key(&edge_id),
            "property row mapping without authority entry"
        );
    }

    fn update_property_index_on_delete(
        &mut self,
        properties: &Option<Vec<(String, Value)>>,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) {
        let Some(ref props) = properties else {
            return;
        };
        if self.property_index.is_none() {
            return;
        }
        let outcomes: Vec<(String, StorageResult<()>, u64)> =
            if let Some(ref mut index) = self.property_index {
                props
                    .iter()
                    .map(|(prop_name, prop_value)| {
                        let started = std::time::Instant::now();
                        let result = index.delete(prop_name, prop_value, src, dst, rank, ts);
                        let latency = started.elapsed().as_millis() as u64;
                        (prop_name.clone(), result, latency)
                    })
                    .collect()
            } else {
                Vec::new()
            };
        for (prop_name, result, latency) in outcomes {
            self.note_index_result(&prop_name, result, latency);
        }
    }

    /// Revert a deletion by edge key without offsets.
    ///
    /// Undo path for transaction rollback: scans every physical generation
    /// sharing the endpoint key, verifies this undo owns the deletion
    /// through the authority, then reverts both directions by edge id.
    /// Authority revival requires both directions to revert; a partial
    /// revert keeps the deletion mark and reports an error.
    pub fn revert_delete_edge(
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
        let mut candidates: Vec<EdgeId> = Vec::new();
        self.out_csr.visit_physical(src, |nbr| {
            if nbr.to_vertex_id() == dst_key {
                candidates.push(nbr.edge_id);
            }
            true
        });
        let mut edge_id = None;
        for candidate in candidates {
            match self.mvcc.edge_timestamps.get(&candidate) {
                Some(info) if info.delete_ts != Timestamp::MAX && info.delete_ts <= ts => {
                    edge_id = Some(candidate);
                    break;
                }
                _ => continue,
            }
        }
        let Some(edge_id) = edge_id else {
            return Ok(false);
        };
        if !self.out_csr.revert_delete_by_edge_id(src, edge_id, ts) {
            return Ok(false);
        }
        if !self.in_csr.revert_delete_by_edge_id(dst, edge_id, ts) {
            return Err(StorageError::invalid_operation(format!(
                "delete rollback failed for edge {:?}: in-direction revert missed",
                edge_id
            )));
        }
        if let Some(ts_info) = self.mvcc.edge_timestamps.get_mut(&edge_id) {
            ts_info.delete_ts = Timestamp::MAX;
        }
        let _ = self.properties.revert_deletion_for_edge(edge_id);
        let restored = self.properties_for_edge(edge_id, ts);
        if self.property_index.is_some() {
            let label = self.label;
            let outcomes: Vec<(String, StorageResult<()>, u64)> =
                if let Some(ref mut index) = self.property_index {
                    restored
                        .into_iter()
                        .map(|(prop_name, prop_value)| {
                            let started = std::time::Instant::now();
                            let result =
                                index.insert(&prop_name, &prop_value, src, dst, rank, label, ts);
                            let latency = started.elapsed().as_millis() as u64;
                            (prop_name, result, latency)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }
        self.mark_properties_dirty();
        self.debug_assert_copies_consistent(edge_id);
        Ok(true)
    }

    /// Resolve the edge id for `(src, dst, rank)` without decoding properties.
    ///
    /// Operation-layer point lookups use it to recheck the fetched record
    /// through the pending-aware gate
    /// (`MVCCManager::is_edge_visible_with_gate`).
    pub fn edge_id_of(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeId> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.merged_get_edge(&self.out_csr, src, dst_key, ts)
            .map(|nbr| nbr.edge_id)
    }

    pub fn get_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.merged_get_edge(&self.out_csr, src, dst_key, ts)?;
        let properties = self.properties_for_edge(nbr.edge_id, ts);

        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn get_edge_with_gate(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.physical_location(&self.out_csr, src, dst_key)?;
        if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
            return None;
        }
        let properties = self.properties_for_edge(nbr.edge_id, ts);
        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn get_edge_with_gate_projected(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.physical_location(&self.out_csr, src, dst_key)?;
        if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
            return None;
        }
        let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        self.out_edges_projected(src, ts, None)
    }

    pub fn out_edges_projected(
        &self,
        src: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs = self.merged_out_nbrs(src, ts);

        nbrs.into_iter()
            .map(|nbr| self.edge_record_from_nbr_projected(src, nbr, ts, projection))
            .collect()
    }

    /// Raw out-edge neighbors of `src` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    pub fn merged_out_nbrs(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        self.merged_edges_of(&self.out_csr, src, ts)
    }

    pub fn in_edges(&self, dst: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        self.in_edges_projected(dst, ts, None)
    }

    pub fn in_edges_projected(
        &self,
        dst: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs = self.merged_in_nbrs(dst, ts);

        nbrs.into_iter()
            .map(|nbr| {
                let src_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);

                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
    }

    /// Raw in-edge neighbors of `dst` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    ///
    /// Allocating convenience; high-frequency paths use the batch accessor.
    pub fn merged_in_nbrs(&self, dst: u32, ts: Timestamp) -> Vec<Nbr> {
        self.merged_edges_of(&self.in_csr, dst, ts)
    }

    /// Batch adjacency accessor bound to one snapshot.
    ///
    /// Traversal and batch queries use the accessor with a caller buffer so
    /// peak memory stays proportional to the batch size instead of the row
    /// degree. The buffer is valid only for the snapshot the accessor was
    /// created with and must not be held across snapshots.
    pub fn batch_accessor(
        &self,
        outgoing: bool,
        ts: Timestamp,
    ) -> super::iterator::AdjacencyBatchAccessor<'_> {
        super::iterator::AdjacencyBatchAccessor::new(self, outgoing, ts)
    }

    /// Whether one edge matches every pushed predicate at `query_ts`.
    ///
    /// Column-scan pushdown: only predicate columns are read through their
    /// null bitmaps, with no intermediate record materialization. Visibility
    /// stays with the authority; callers check it separately.
    pub fn matches_pushdown(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        predicates: &[ScanPredicate],
    ) -> bool {
        self.properties
            .matches_predicates_for_edge(edge_id, query_ts, predicates)
    }

    /// Filter edge ids by pushed predicates at the column-scan layer.
    ///
    /// Hits look up topology afterwards; misses never decode a record.
    /// Index first, segment statistics second, full walk last: an
    /// every-equality conjunction over lag-free indexed columns resolves to
    /// index candidates (verified back against the columns), otherwise whole
    /// owner groups provably excluding the predicates are skipped before the
    /// property walk. A full walk without either aid logs an observation so
    /// operators know an index would help.
    pub fn filter_edge_ids(
        &self,
        predicates: &[ScanPredicate],
        query_ts: Timestamp,
        candidates: Option<&[EdgeId]>,
    ) -> Vec<EdgeId> {
        if let Some(ids) = candidates {
            return self
                .properties
                .filter_edge_ids_by_predicates(predicates, query_ts, Some(ids));
        }
        if predicates.is_empty() {
            return self
                .properties
                .filter_edge_ids_by_predicates(predicates, query_ts, None);
        }
        if let Some(indexed) = self.index_candidate_edge_ids(predicates, query_ts) {
            return self.properties.filter_edge_ids_by_predicates(
                predicates,
                query_ts,
                Some(&indexed),
            );
        }
        let pruned = self.pruned_owner_groups(predicates);
        if pruned.is_empty() {
            log::debug!(
                "filter_edge_ids: no usable index or segment prune, full property walk over {} rows",
                self.properties.row_count()
            );
            return self
                .properties
                .filter_edge_ids_by_predicates(predicates, query_ts, None);
        }
        let survivors: Vec<EdgeId> = self
            .properties
            .edge_ids()
            .filter(|edge_id| {
                self.edge_owner
                    .get(edge_id)
                    .is_none_or(|owner| !pruned.contains(owner))
            })
            .collect();
        self.properties
            .filter_edge_ids_by_predicates(predicates, query_ts, Some(&survivors))
    }

    /// Owner groups provably excluding the pushed predicates.
    ///
    /// Dirty groups never prune: their uncheckpointed writes are not covered
    /// by the flushed statistics, mirroring `segment_may_contain`.
    fn pruned_owner_groups(&self, predicates: &[ScanPredicate]) -> HashSet<u32> {
        let mut pruned = HashSet::new();
        for gid in self.owner_group_ids() {
            if !self.segment_may_contain(gid, predicates) {
                pruned.insert(gid);
            }
        }
        pruned
    }

    /// Candidate edges from the secondary property index for one filter.
    ///
    /// Serves only every-equality conjunctions whose columns all carry an
    /// index, and only while the index carries no write lag: the index is
    /// best-effort, so any recorded failure falls back to the segment path
    /// instead of risking dropped hits. Stale entries resolve through the
    /// visibility authority, and the caller verifies every candidate back
    /// against the property columns.
    fn index_candidate_edge_ids(
        &self,
        predicates: &[ScanPredicate],
        query_ts: Timestamp,
    ) -> Option<Vec<EdgeId>> {
        let index = self.property_index.as_ref()?;
        if self.index_write_failures > 0 {
            return None;
        }
        let mut merged: Option<HashSet<EdgeId>> = None;
        for predicate in predicates {
            let (column, value) = match predicate {
                ScanPredicate::ColumnEqual { column, value } => (column, value),
                ScanPredicate::ColumnRange { .. } => return None,
            };
            if !index.has_index(column) {
                return None;
            }
            let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
            let (lower, upper) = codec.prefix_bounds(value).ok()?;
            let mut hits = HashSet::new();
            for ((src, dst, rank), _) in index.lookup(column, &lower, &upper) {
                if let Some(edge_id) = self.edge_id_of(src, dst, rank, query_ts) {
                    hits.insert(edge_id);
                }
            }
            merged = Some(match merged {
                None => hits,
                Some(prev) => prev.intersection(&hits).copied().collect(),
            });
            if merged.as_ref().is_some_and(HashSet::is_empty) {
                break;
            }
        }
        merged.map(|set| set.into_iter().collect())
    }

    /// Whether one owner group may contain rows matching the predicates.
    ///
    /// Dirty groups always scan: their uncheckpointed writes are not covered
    /// by the flushed statistics. Clean groups prune only on provable
    /// exclusion from the widened bounds, so the pre-filter never changes
    /// results.
    pub fn segment_may_contain(&self, group: u32, predicates: &[ScanPredicate]) -> bool {
        if predicates.is_empty() {
            return true;
        }
        let gid = group as usize;
        if self.out_csr.needs_checkpoint(gid) || self.in_csr.needs_checkpoint(gid) {
            return true;
        }
        let Some(stats) = self.segment_stats.get(&group) else {
            return true;
        };
        stats.may_contain(predicates)
    }

    /// Snapshot of the in-memory segment statistics.
    pub fn segment_stats_snapshot(&self) -> HashMap<u32, GroupSegmentStats> {
        self.segment_stats.clone()
    }

    /// Restore segment statistics decoded from a checkpoint.
    pub(crate) fn restore_segment_stats(&mut self, stats: HashMap<u32, GroupSegmentStats>) {
        self.segment_stats = stats;
    }

    /// Collect fresh per-group segment statistics and widen the in-memory
    /// snapshot. Only groups touched since the last checkpoint are
    /// recollected; clean groups keep their previous snapshot so checkpoint
    /// cost follows dirty groups rather than table size. Bounds widen
    /// monotonically so pruning stays conservative; counts are exact-current.
    pub(crate) fn refresh_segment_stats(&mut self) {
        use std::collections::{HashMap, HashSet};
        let use_out = self.schema.oe_strategy != super::super::EdgeStrategy::None;
        let existing: Vec<u32> = if use_out {
            self.out_csr.existing_group_ids()
        } else {
            self.in_csr.existing_group_ids()
        }
        .into_iter()
        .map(|gid| gid as u32)
        .collect();
        let live_set: HashSet<u32> = existing.iter().copied().collect();
        self.segment_stats
            .retain(|group, _| live_set.contains(group));
        let mut dirty: HashSet<u32> = HashSet::new();
        for gid in self.out_csr.dirty_group_ids() {
            dirty.insert(gid as u32);
        }
        for gid in self.in_csr.dirty_group_ids() {
            dirty.insert(gid as u32);
        }
        for gid in self.out_csr.sampled_column_dirty_group_ids() {
            dirty.insert(gid as u32);
        }
        for gid in self.in_csr.sampled_column_dirty_group_ids() {
            dirty.insert(gid as u32);
        }
        for gid in &existing {
            if !self.segment_stats.contains_key(gid) {
                dirty.insert(*gid);
            }
        }
        dirty.retain(|gid| live_set.contains(gid));
        if dirty.is_empty() {
            return;
        }
        let group_ids: Vec<u32> = dirty.into_iter().collect();
        let group_size = if use_out {
            self.out_csr.group_size()
        } else {
            self.in_csr.group_size()
        } as u64;
        let column_names: Vec<String> = self
            .schema
            .properties
            .iter()
            .map(|prop| prop.name.clone())
            .collect();
        let mut encodings: HashMap<String, crate::encoding::EncodingType> = HashMap::new();
        for name in &column_names {
            if let Some(encoding) = self.properties.column_encoding_type(name) {
                encodings.insert(name.clone(), encoding);
            }
        }
        let mut by_owner: HashMap<u32, Vec<EdgeId>> = HashMap::new();
        for group in &group_ids {
            by_owner.insert(*group, Vec::new());
        }
        for edge_id in self.properties.edge_ids() {
            let owner = self.edge_owner.get(&edge_id).copied().unwrap_or(0);
            if let Some(slot) = by_owner.get_mut(&owner) {
                slot.push(edge_id);
            }
        }
        for group in group_ids {
            let gid = group as usize;
            let live = if use_out {
                self.out_csr.group_live_count(gid)
            } else {
                self.in_csr.group_live_count(gid)
            };
            let owned = by_owner.get(&group);
            let mut endpoints: Vec<u32> = Vec::new();
            if let Some(variant) = if use_out {
                self.out_csr.group_variant(gid)
            } else {
                self.in_csr.group_variant(gid)
            } {
                for (_, nbr) in variant.iter_all() {
                    endpoints.push(nbr.endpoint);
                }
            }
            let mut column_values: HashMap<String, Vec<Option<Value>>> = HashMap::new();
            for name in &column_names {
                column_values.insert(name.clone(), Vec::new());
            }
            if let Some(edges) = owned {
                for edge_id in edges {
                    if let Some(cells) = self.properties.get_projected_physical_by_edge_id(
                        *edge_id,
                        graphdb_core::types::MAX_TIMESTAMP,
                        None,
                    ) {
                        let cell_map: HashMap<&String, &Option<Value>> =
                            cells.iter().map(|(name, value)| (name, value)).collect();
                        for name in &column_names {
                            let value = cell_map.get(name).and_then(|cell| (*cell).clone());
                            if let Some(slot) = column_values.get_mut(name) {
                                slot.push(value);
                            }
                        }
                    }
                }
            }
            let fresh = GroupSegmentStats::collect(
                group,
                group_size,
                live,
                &endpoints,
                &column_values,
                &encodings,
            );
            match self.segment_stats.get_mut(&group) {
                Some(current) => current.widen_with(&fresh),
                None => {
                    self.segment_stats.insert(group, fresh);
                }
            }
        }
    }

    /// Encoding report for the persisted topology columns of both
    /// directions. Neighbor and edge-id columns come first, offset and
    /// length columns follow; all use the integer column path only.
    pub fn topology_encoding_report(
        &self,
    ) -> Vec<(
        String,
        crate::edge::mutable_csr::serialization::TopologyColumnEncoding,
        usize,
        usize,
    )> {
        let mut report = Vec::new();
        for gid in self.out_csr.existing_group_ids() {
            if let Some(variant) = self.out_csr.group_variant(gid) {
                if let super::super::CsrVariant::Multiple(csr) = variant {
                    for (name, encoding, plain, encoded) in csr.topology_encoding_report() {
                        report.push((format!("out_g{}:{}", gid, name), encoding, plain, encoded));
                    }
                }
            }
        }
        for gid in self.in_csr.existing_group_ids() {
            if let Some(variant) = self.in_csr.group_variant(gid) {
                if let super::super::CsrVariant::Multiple(csr) = variant {
                    for (name, encoding, plain, encoded) in csr.topology_encoding_report() {
                        report.push((format!("in_g{}:{}", gid, name), encoding, plain, encoded));
                    }
                }
            }
        }
        report
    }

    pub fn has_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        if !self.is_open {
            return false;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.merged_get_edge(&self.out_csr, src, dst_key, ts)
            .is_some()
    }

    pub fn edge_count(&self) -> u64 {
        self.out_csr.edge_count()
    }

    pub fn delta_edge_count(&self) -> u64 {
        self.out_csr.edge_count() + self.in_csr.edge_count()
    }

    pub fn scan(&self, ts: Timestamp) -> Vec<EdgeRecord> {
        self.scan_projected(ts, None)
    }

    pub fn scan_projected(
        &self,
        ts: Timestamp,
        projection: Option<Vec<String>>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        self.iter_projected(ts, projection).collect()
    }

    /// Batch full-table scan with a pending gate. Materializes every visible
    /// record, so it is an offline path for index builds and scatter-gather
    /// queries; latency-sensitive traversals should use the streaming
    /// [`EdgeStore::iter`] plus per-vertex limit pushdown instead.
    pub fn scan_with_gate(
        &self,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        let mut records = Vec::new();
        for (src_vid, nbr) in self.out_csr.iter_all() {
            if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                continue;
            }
            records.push(self.edge_record_from_nbr(
                src_vid.as_int64().unwrap_or(0) as u32,
                nbr,
                ts,
            ));
        }
        records
    }

    pub fn scan_with_gate_projected(
        &self,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        let mut records = Vec::new();
        for (src_vid, nbr) in self.out_csr.iter_all() {
            if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                continue;
            }
            records.push(self.edge_record_from_nbr_projected(
                src_vid.as_int64().unwrap_or(0) as u32,
                nbr,
                ts,
                projection,
            ));
        }
        records
    }

    /// Optimizer-facing statistics snapshot for one property column
    /// (zone-map aggregated, with live row count).
    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        self.properties.column_stats_snapshot(column)
    }

    /// Record a schema change event
    ///
    /// Handles the common pattern of:
    /// 1. Computing next version number from history
    /// 2. Creating a PropertyChange event
    /// 3. Recording it in the version history
    pub(crate) fn record_schema_change(&mut self, details: ChangeDetails) -> StorageResult<()> {
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
        // Single code path: the immediate add is prepare + fill + publish of
        // the staged state machine, so no second column-construction
        // implementation exists.
        self.prepare_add_property(name.clone(), data_type.clone(), nullable, None)?;
        if let Err(error) = self.fill_pending_add_property() {
            let _ = self.abort_pending_add_property();
            return Err(error);
        }
        self.publish_pending_add_property()
    }

    /// Select and apply encodings for every property column.
    ///
    /// Explicit maintenance operation: hot columns stay unencoded between
    /// runs by design so everyday writes never pay re-encoding. Encodings
    /// change the physical representation without changing logical values,
    /// and the encoding choice is part of the checkpoint payload, so this
    /// marks properties dirty to carry the choice to the next checkpoint.
    /// Returns the number of columns that received an encoding.
    pub fn encode_property_columns(&mut self) -> usize {
        let encoded = self.properties.auto_encode_properties();
        if encoded > 0 {
            self.mark_properties_dirty();
        }
        encoded
    }

    /// Recompute persisted per-column statistics from current contents.
    pub fn refresh_property_stats(&mut self) {
        self.properties.refresh_column_stats();
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
        // Single code path: the immediate drop is prepare + publish of the
        // staged state machine, so no second column-removal implementation
        // exists. Publish restores from its snapshot when history recording
        // fails; other failures leave storage untouched.
        let owned = name.to_string();
        self.prepare_drop_property(&owned)?;
        if let Err(error) = self.publish_pending_drop_property() {
            let _ = self.abort_pending_drop_property();
            return Err(error);
        }
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

        if let Err(error) = self.record_schema_change(ChangeDetails::PropertyRenamed {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        }) {
            // History is the last step: rename everything back so no
            // half-renamed state survives a history failure.
            let _ = self.properties.rename_property(new_name, old_name);
            self.schema.properties[index].name = old_name.to_string();
            if let Some(idx) = self.property_index_cache.remove(new_name) {
                self.property_index_cache.insert(old_name.to_string(), idx);
            }
            return Err(error);
        }
        self.mark_properties_dirty();

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
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, src, dst_key, ts) {
            if let Some(dir) = self.wal_dir.clone() {
                super::wal::append_ops(
                    &dir,
                    &[super::wal::EdgeWalOp::PropertyUpdate {
                        src,
                        dst,
                        rank,
                        prop_name: prop_name.to_string(),
                        value: value.clone(),
                        ts,
                    }],
                )?;
            }
            self.properties
                .set_property_for_edge(nbr.edge_id, prop_name, Some(value.clone()), ts)
                .map_err(|_| StorageError::column_not_found(prop_name.to_string()))?;
            self.mark_properties_dirty_for_edge(src, dst);
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    pub fn update_edge_property_by_key(
        &mut self,
        params: UpdateEdgePropertyByKeyParams,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        let dst_key = Self::edge_endpoint_key(params.dst, params.rank);
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, params.src, dst_key, params.ts) {
            if let Some(dir) = self.wal_dir.clone() {
                let prop_name = self
                    .properties
                    .property_schema()
                    .iter()
                    .find(|schema| schema.prop_id as u16 == params.prop_id)
                    .map(|schema| schema.name.clone())
                    .unwrap_or_else(|| format!("prop_id={}", params.prop_id));
                super::wal::append_ops(
                    &dir,
                    &[super::wal::EdgeWalOp::PropertyUpdate {
                        src: params.src,
                        dst: params.dst,
                        rank: params.rank,
                        prop_name,
                        value: params.value.clone(),
                        ts: params.ts,
                    }],
                )?;
            }
            self.properties
                .set_property_by_id_for_edge(
                    nbr.edge_id,
                    PropertyId(params.prop_id),
                    Some(params.value.clone()),
                    params.ts,
                )
                .map_err(|_| {
                    StorageError::column_not_found(format!("prop_id={}", params.prop_id))
                })?;
            self.mark_properties_dirty_for_edge(params.src, params.dst);

            let src_key = Self::edge_endpoint_key(params.src, params.rank);
            if let Some(ie_nbr) = self.merged_get_edge(&self.in_csr, params.dst, src_key, params.ts)
            {
                if nbr.edge_id != ie_nbr.edge_id {
                    return Err(StorageError::data_corruption(format!(
                        "edge_id mismatch: out_csr={}, in_csr={} at edge ({}, {})",
                        nbr.edge_id.0, ie_nbr.edge_id.0, params.src, params.dst
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

    pub fn iter_projected(
        &self,
        ts: Timestamp,
        projection: Option<Vec<String>>,
    ) -> EdgeTableScanIterator<'_> {
        EdgeTableScanIterator::with_projection(self, ts, projection)
    }

    pub fn memory_size(&self) -> usize {
        self.used_memory_size()
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = 0;

        total += self.out_csr.used_memory_size();
        total += self.in_csr.used_memory_size();
        // Authority records share the tombstone per-record estimate so live
        // and deleted entries use one caliber including hash overhead.
        total += super::stats::TombstoneStats::estimate_memory(self.mvcc.edge_timestamps.len());
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
    ///
    /// Write-path fast path: counts plus per-shard sizes only, never a
    /// full-table fragmentation walk. A zero per-edge measurement reports a
    /// zero estimate explicitly; backpressure treats zero as no pressure and
    /// never divides by the estimate.
    pub fn estimate_memory_usage(&self) -> usize {
        let out_edges = self.out_csr.edge_count() as usize;
        let in_edges = self.in_csr.edge_count() as usize;
        let out_bytes_per_edge = self.out_csr.bytes_per_edge();
        let in_bytes_per_edge = self.in_csr.bytes_per_edge();
        out_edges * out_bytes_per_edge + in_edges * in_bytes_per_edge
    }

    /// Record mutable CSR pressure without performing maintenance on the write path.
    pub fn check_and_apply_write_backpressure(&mut self, _current_ts: Timestamp) -> bool {
        if self.config.max_mutable_csr_bytes == 0 {
            return false; // Backpressure disabled
        }

        let mutable_size = self.estimate_memory_usage();

        if mutable_size > self.config.max_mutable_csr_bytes {
            return true;
        }

        false
    }

    pub fn needs_background_maintenance(&self) -> bool {
        self.config.max_mutable_csr_bytes > 0
            && self.estimate_memory_usage() > self.config.max_mutable_csr_bytes
    }

    /// Write-path fast path: bound comes from the per-table pin cache, with
    /// no global watermark capture. Background passes use the watermark
    /// variant below.
    pub fn maybe_run_auto_maintenance(&mut self) -> usize {
        let bound = self.mvcc.min_active_snapshot_ts;
        self.run_auto_maintenance_pass(bound)
    }

    fn run_auto_maintenance_pass(&mut self, bound: Timestamp) -> usize {
        let cfg = self.config.auto_maintenance;
        let mut maintenance_ran = 0;

        // Reclaim edge-property before-images that no active snapshot can
        // observe. Version history is memory-only (checkpoints persist
        // current values), so reclamation never dirties the property file.
        if bound != Timestamp::MAX {
            let removed = self.properties.gc_property_versions(bound);
            if removed > 0 {
                maintenance_ran += 1;
            }
        }

        if cfg.property_compact_ratio > 0.0 && bound != Timestamp::MAX {
            let prop_stats = self.properties.compaction_stats();
            if prop_stats.fragmentation_ratio() >= cfg.property_compact_ratio as f64 {
                self.compact_properties(bound);
                maintenance_ran += 1;
            }
        }

        // Incremental row reclaim on the write path: only rows holding
        // entries eligible at this cutoff are visited, and each pass stops
        // after a bounded row count so small writes never cause large
        // rebuilds. Skipped entirely while no tombstone exists.
        if self.run_vertex_reclaim_pass(bound) {
            maintenance_ran += 1;
        }

        maintenance_ran
    }

    /// Background variant: bound comes from one per-pass watermark capture
    /// shared across all tables.
    pub fn maybe_run_auto_maintenance_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        let bound = watermarks.safe_gc_timestamp_with_margin(margin);
        self.run_auto_maintenance_pass(bound)
    }

    // ── Edge Property Index ──

    /// Enable property index with the specified pool capacity.
    /// Builds the index from existing edge data.
    pub fn enable_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        self.build_property_index(pool_capacity)
    }

    /// Build the property index by scanning all edges.
    /// Streams one record at a time so peak memory stays flat instead of
    /// materializing every live edge plus decoded properties at once.
    /// The fresh scan resets the lag baseline: the failure counter ends at
    /// the build's own failure count, never silently cleared to zero.
    pub(crate) fn build_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        let mut index = EdgePropertyIndex::new(pool_capacity);
        // MAX_TIMESTAMP satisfies `create_ts <= ts < delete_ts` for live
        // edges, so all non-tombstoned edges are scanned.
        let all_ts = graphdb_core::types::MAX_TIMESTAMP;
        let label = self.label;

        let iter = EdgeTableScanIterator::new(self, all_ts);
        let mut build_failures: u64 = 0;
        let space_id = self.label as u64;
        let stats_manager = self.stats_manager.clone();
        for edge in iter {
            let src_u32 = edge.src_vid.as_int64().unwrap_or(0) as u32;
            let dst_u32 = edge.dst_vid.as_int64().unwrap_or(0) as u32;
            for (prop_name, prop_value) in &edge.properties {
                let started = std::time::Instant::now();
                let result = index.insert(
                    prop_name, prop_value, src_u32, dst_u32, edge.rank, label, all_ts,
                );
                let latency = started.elapsed().as_millis() as u64;
                if result.is_err() {
                    build_failures = build_failures.saturating_add(1);
                    if let Some(stats) = &stats_manager {
                        stats.record_index_operation(space_id, prop_name, latency, false);
                    }
                } else if let Some(stats) = &stats_manager {
                    stats.record_index_operation(space_id, prop_name, latency, true);
                }
            }
        }

        self.property_index = Some(index);
        self.index_write_failures = build_failures;
        if build_failures > 0 {
            log::debug!(
                "build_property_index: {} secondary writes failed, lag counter carries them",
                build_failures
            );
        }
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

    pub fn flush<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
        compression: crate::compression::CompressionType,
    ) -> StorageResult<crate::edge::EdgeCheckpointKind> {
        let path = path.as_ref();
        let crate::compression::CompressionType::Zstd { level } = compression;
        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        self.flush_incremental(path, page_size, level)
    }

    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        self.load_incremental(path.as_ref())
    }

    /// Fail-closed cross-copy audit used by [`EdgeStore::load`].
    ///
    /// Damage detection only: returns `(orphan property mappings, orphan CSR
    /// rows, live authority orphans)`. A nonzero count signals corrupt files
    /// or a write-path regression; callers reject the load. Crash consistency
    /// itself comes from the checkpoint commit protocol (groups before
    /// metadata, manifest published last with its tail embedded in
    /// `meta.bin`), not from this audit.
    ///
    /// One CSR traversal feeds both the orphan-row count and the live
    /// authority check, so the load path pays a single pass over both
    /// directions instead of two.
    pub(crate) fn copy_audit(&self) -> (usize, usize, usize) {
        let mut csr_ids = HashSet::new();
        let mut orphan_csr_rows = 0;
        for (_, nbr) in self.out_csr.iter_all().chain(self.in_csr.iter_all()) {
            if !self.mvcc.edge_timestamps.contains_key(&nbr.edge_id) {
                orphan_csr_rows += 1;
            }
            csr_ids.insert(nbr.edge_id);
        }
        let orphan_mappings = self
            .properties
            .edge_ids()
            .filter(|edge_id| !self.mvcc.edge_timestamps.contains_key(edge_id))
            .count();
        let live_orphans = self
            .mvcc
            .edge_timestamps
            .iter()
            .filter(|(edge_id, ts)| ts.delete_ts == Timestamp::MAX && !csr_ids.contains(edge_id))
            .count();
        (orphan_mappings, orphan_csr_rows, live_orphans)
    }

    /// Orphan property mappings plus orphan CSR rows.
    /// Audit-only (load path plus tests); a nonzero count means corrupt files
    /// or a write-path regression.
    ///
    /// Live-authority orphans (a live authority entry with no CSR row, as
    /// produced by a silent Single-slot overwrite) are reported separately
    /// by [`EdgeStore::live_authority_orphans`] and also reject the load.
    pub fn loaded_copy_mismatches(&self) -> (usize, usize) {
        let (orphan_mappings, orphan_csr_rows, _) = self.copy_audit();
        (orphan_mappings, orphan_csr_rows)
    }

    /// Live authority entries with no CSR row in either direction.
    /// Audit-only (load path plus tests); a nonzero count means a past
    /// silent overwrite orphaned the authority record.
    pub fn live_authority_orphans(&self) -> usize {
        let (_, _, live_orphans) = self.copy_audit();
        live_orphans
    }

    /// Replay one write-ahead log operation idempotently.
    ///
    /// Called only during load recovery with `wal_dir` cleared so replayed
    /// commits never append back to the log. Inserts skip on
    /// `EdgeAlreadyExists`, deletes treat missing edges as done, updates
    /// overwrite the same value, and schema changes skip when already
    /// applied, so a repeated replay yields the same state.
    pub(crate) fn replay_one_wal_op(
        &mut self,
        op: super::wal::EdgeWalOp,
    ) -> StorageResult<()> {
        match op {
            super::wal::EdgeWalOp::Insert {
                src,
                dst,
                rank,
                properties,
                create_ts,
            } => match self.insert_edge(src, dst, rank, &properties, create_ts) {
                Ok(()) => Ok(()),
                Err(e)
                    if e.kind()
                        == graphdb_core::error::storage::StorageErrorKind::EdgeAlreadyExists =>
                {
                    Ok(())
                }
                Err(e) => Err(e),
            },
            super::wal::EdgeWalOp::Delete {
                src,
                dst,
                rank,
                delete_ts,
            } => match self.delete_edge(src, dst, rank, delete_ts) {
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            },
            super::wal::EdgeWalOp::PropertyUpdate {
                src,
                dst,
                rank,
                prop_name,
                value,
                ts,
            } => {
                self.update_edge_property(src, dst, rank, &prop_name, &value, ts)?;
                Ok(())
            }
            super::wal::EdgeWalOp::SchemaAdd {
                name,
                data_type,
                nullable,
                default,
            } => {
                if self.properties.has_property(&name) {
                    return Ok(());
                }
                self.prepare_add_property(name.clone(), data_type, nullable, default)?;
                if let Err(e) = self.fill_pending_add_property() {
                    let _ = self.abort_pending_add_property();
                    return Err(e);
                }
                match self.publish_pending_add_property() {
                    Ok(()) => Ok(()),
                    Err(e)
                        if e.kind()
                            == graphdb_core::error::storage::StorageErrorKind::ColumnAlreadyExists =>
                    {
                        let _ = self.abort_pending_add_property();
                        Ok(())
                    }
                    Err(e) => {
                        let _ = self.abort_pending_add_property();
                        Err(e)
                    }
                }
            }
            super::wal::EdgeWalOp::SchemaDrop { name } => {
                if !self.properties.has_property(&name) {
                    return Ok(());
                }
                match self.remove_property(&name) {
                    Ok(()) => Ok(()),
                    Err(e)
                        if e.kind()
                            == graphdb_core::error::storage::StorageErrorKind::ColumnNotFound =>
                    {
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
