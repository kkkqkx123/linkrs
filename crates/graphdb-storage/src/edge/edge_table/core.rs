//! Core EdgeStore table definition with per-responsibility operation modules.
//!
//! Node-group sharded edge table: one sharded CSR per direction plus
//! centralized row-level timestamps. There are no frozen segments, no
//! merges, and no cross-segment deduplication.
//!
//! Organization (`core/`):
//! - `store`: construction and identity accessors
//! - `owner`: owner-group shard routing
//! - `reads`: visibility, adjacency, point lookups and scans
//! - `writes`: staging commit, insert, delete and undo
//! - `index`: secondary property index maintenance
//! - `query`: predicate pushdown and segment pruning statistics
//! - `schema_ops`: schema evolution and column maintenance
//! - `maintenance`: resource accounting, backpressure and upkeep
//! - `recovery`: checkpoint wrappers, audit and WAL replay
//!
//! Concurrency: the table serializes writers and adds no locks around the
//! CSR stack; the canonical discipline lives in `edge::mutable_csr` and is
//! not restated here. The `version_history` mutex guards label history
//! metadata only, never topology.

use super::super::{CsrShardSet, EdgeSchema};
use super::mvcc::MVCCManager;
use super::schema_add_column::PendingAddColumn;
use super::schema_drop_column::PendingDropColumn;
use super::stats::GroupSegmentStats;
use crate::edge::CsrWithProperties;
use crate::edge::IndexConsistency;
use crate::index::edge_index_manager::EdgePropertyIndex;
use crate::schema::LabelVersionHistory;
use graphdb_core::types::{EdgeId, LabelId, Timestamp};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub use super::config::{AutoMaintenanceConfig, EdgeTableConfig, UpdateEdgePropertyByKeyParams};
pub use super::iterator::EdgeTableScanIterator;
pub use writes::IncidentDeletedEdge;

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
    /// Dirty property columns by owner group, keyed by the same owner id
    /// that names the `props_g{gid}` shards. Narrows the incremental
    /// property patch to the columns each group actually touched; groups
    /// without an entry fall back to the table-wide dirty set. Cleared per
    /// group by the flush that persists it, and fully on load or rebuild.
    pub(crate) property_column_dirt: HashMap<u32, HashSet<String>>,
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
    /// Consistency contract of the secondary index. Best-effort counts
    /// failures as lag; Strong fails the primary write on index error.
    pub index_consistency: IndexConsistency,
    /// Lag watermark: failure count at the last successful rebuild or reset.
    /// Queries compare the current counter against this baseline to decide
    /// whether the index is safe to use.
    pub index_lag_baseline: u64,
    /// Pool capacity the secondary index was built with, reused by automatic
    /// rebuilds so maintenance needs no caller-provided capacity.
    pub(crate) index_pool_capacity: u64,
    /// Wall-clock moment the index first lagged since the last rebuild or
    /// reset. Drives the max-staleness rebuild trigger; cleared on rebuild.
    pub(crate) index_stale_since: Option<std::time::Instant>,
    /// Committed write counts by owner group for hotspot observability.
    /// Bounded by the group count; the write-hot path only increments.
    pub(crate) group_write_counts: HashMap<u32, u64>,

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
    /// reshard; orphan timestamps without topology converge to the first
    /// existing group and are counted as relocated orphans.
    pub(crate) edge_owner: owner::EdgeOwnerMap,
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
    /// Source: the skip is purely an optimization over the watermark-driven
    /// pass in `core/maintenance.rs`; correctness never depends on it because
    /// any tombstone growth re-arms the pass via the baseline below.
    pub(crate) last_reclaim_bound: Timestamp,
    /// Tombstone count seen by the last executed reclaim pass. Growth past
    /// this baseline re-arms the pass even when the watermark stands still.
    pub(crate) last_reclaim_tombstones: usize,
    /// Directory of the last checkpoint or load, owning the write-ahead log.
    /// Commits append logical redo here before returning success; checkpoints
    /// truncate it after the new snapshot is durable. `None` before the first
    /// checkpoint, when redo has no home yet.
    pub(crate) wal_dir: Option<std::path::PathBuf>,
    /// Table-level property fallback rewrites since creation.
    ///
    /// Counts checkpoints where property dirt carried no group trace and every
    /// owner rewrote as insurance. The regular path always carries a trace,
    /// so this counter stays flat under normal load and any growth points at
    /// a missing write-time mark.
    pub(crate) property_fallback_rewrites: u64,
    /// In-flight staged rename-column change. Memory-only, same crash contract
    /// as the staged add and drop: at most one schema change is pending at a
    /// time and a crash before publishing is equivalent to aborting.
    pub(crate) pending_rename_column: Option<super::schema_rename_column::PendingRenameColumn>,
    /// A record-form switch completed since the last checkpoint. Pre-switch
    /// WAL redo is fenced at switch time and must never replay onto the new
    /// form, so the next checkpoint is mandatory, not advisory. Memory-only.
    pub(crate) migration_pending_checkpoint: bool,
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

mod index;
mod maintenance;
pub(crate) mod owner;
mod query;
mod reads;
mod recovery;
mod schema_ops;
mod store;
mod writes;

#[cfg(test)]
#[path = "core_tests/mod.rs"]
mod tests;
