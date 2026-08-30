use crate::index::helpers::{
    edge_entity_ref, flush_split_generation, merge_split_wal_changes, vertex_entity_ref,
};
use crate::index::key_codec::key_types::{
    SecondaryIndexKey, KEY_TYPE_EDGE_REVERSE, KEY_TYPE_VERTEX_REVERSE,
};
use crate::index::key_codec::{KeyBuilder, KeyParser};
use crate::index::manifest::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard, ManifestCatalog,
    ManifestHandle,
};
use crate::index::shard_runtime::{
    generation_from_maps_with_pool_capacity, GenerationRuntime, IndexBarrierRegistry, IndexMaps,
    IndexRuntime,
};
use crate::index::types::{EdgeIdentity, IndexIdentity, IndexRecord};
use crate::persistence::{read_versioned_payload, write_versioned_payload};
use graphdb_core::stats::StatsManager;
use graphdb_core::types::{
    CommitLsn, Index, IndexGeneration, IndexType, SnapshotTimestamp, Timestamp,
};
use graphdb_core::value::ordered_codec::OrderedCodec;
use graphdb_core::wal::{EntityRef, OutboxIntent};
use graphdb_core::{StorageError, StorageResult, Value};
use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub struct IndexDataManagerImpl {
    pub(crate) index_root: Option<PathBuf>,
    pub(crate) manifest_catalogs: Arc<RwLock<HashMap<IndexIdentity, Arc<ManifestCatalog>>>>,
    pub(crate) runtimes: Arc<RwLock<HashMap<IndexIdentity, Arc<IndexRuntime>>>>,
    pub(crate) index_aliases: Arc<RwLock<HashMap<(u64, String), u64>>>,
    pub(crate) index_types: Arc<RwLock<HashMap<IndexIdentity, IndexType>>>,
    pub(crate) index_definitions: Arc<RwLock<HashMap<IndexIdentity, Index>>>,
    pub(crate) restored_generations: Arc<RwLock<HashMap<IndexIdentity, IndexGeneration>>>,
    pub(crate) barrier_registry: IndexBarrierRegistry,
    pub(crate) rebuild_gate: Arc<RwLock<()>>,
    pub(crate) stats_manager: Option<Arc<StatsManager>>,
    /// Memory limit in bytes for all indexes. 0 means unlimited.
    pub(crate) memory_limit_bytes: Arc<AtomicU64>,
    /// Cached total memory usage for fast check_memory_limit
    pub(crate) total_memory_usage: Arc<AtomicU64>,
    /// Per-shard buffer pool capacity in bytes.
    pub(crate) pool_capacity: Arc<AtomicU64>,
    /// Enable chunk-level eviction under memory pressure.
    pub(crate) eviction_enabled: Arc<AtomicBool>,
    /// Eviction high-water ratio (stored as ratio * 10000 as integer).
    pub(crate) eviction_high_ratio: Arc<AtomicU64>,
    /// Eviction low-water ratio (stored as ratio * 10000 as integer).
    pub(crate) eviction_low_ratio: Arc<AtomicU64>,
    /// Cached total tombstone count, maintained incrementally on the write path
    /// and resynced by the (rare) GC/retirement/compaction paths. Keeps the
    /// per-statement admission check from scanning every generation.
    pub(crate) cached_tombstone_count: Arc<AtomicU64>,
    /// per-index deltas awaiting publication into a new generation.
    ///
    /// Writes accumulate here (O(1) per statement) instead of publishing a new
    /// generation per statement. The pending delta is folded into a fresh
    /// generation when the entry count reaches `delta_publish_threshold` or
    /// when a read needs a stable snapshot (`publish_pending_delta`).
    pub(crate) pending_deltas: Arc<Mutex<HashMap<IndexIdentity, PendingDelta>>>,
    /// number of pending entries that triggers publication of a new
    /// generation. A value of 0 or 1 disables accumulation, restoring the
    /// per-statement publish behavior (rollback path).
    pub(crate) delta_publish_threshold: Arc<AtomicUsize>,
}

/// Accumulated index deltas awaiting publication into a new generation.
#[derive(Debug, Default, Clone)]
pub(crate) struct PendingDelta {
    /// Per-shard forward/reverse maps with FULL (prefix-included) keys.
    pub(crate) per_shard: HashMap<u32, IndexMaps>,
    /// Number of entries accumulated (sum of forward + reverse keys).
    pub(crate) entries: usize,
    /// Latest write timestamp among the accumulated entries.
    pub(crate) write_ts: Timestamp,
}

/// Mutable accumulators for a pending-delta existing-value scan.
pub(crate) struct PendingExistingScan<'a> {
    pub existing_values: &'a mut Vec<Value>,
    pub existing_encoded: &'a mut HashSet<Vec<u8>>,
    pub existing_columns: &'a mut Vec<(String, Value)>,
    pub covering_populated: &'a mut bool,
}

/// Merge pending-delta reverse entries for `[reverse_prefix, reverse_end)`
/// into the caller's existing-value scan, so the write path observes
/// previously accumulated (but not yet published) entries when computing the
/// diff for a re-written entity.
pub(crate) fn merge_pending_existing_values(
    pending: &PendingDelta,
    reverse_prefix: &[u8],
    reverse_end: &[u8],
    write_ts: Timestamp,
    is_edge: bool,
    scan: &mut PendingExistingScan<'_>,
) {
    use std::ops::Bound;
    for (_, rev_map) in pending.per_shard.values() {
        for (key, record) in rev_map.range((
            Bound::Included(reverse_prefix.to_vec()),
            Bound::Excluded(reverse_end.to_vec()),
        )) {
            if !record.is_visible_at(write_ts) {
                continue;
            }
            let extracted = if is_edge {
                KeyParser::extract_value_from_edge_reverse_suffix(key)
            } else {
                KeyParser::extract_value_from_reverse_suffix(key)
            };
            if let Ok(encoded) = extracted {
                if scan.existing_encoded.insert(encoded.clone()) {
                    if let Ok(value) = OrderedCodec::new().decode(&encoded) {
                        scan.existing_values.push(
                            crate::index::key_codec::key_builder::normalize_int_value(&value),
                        );
                    }
                }
            }
            if !*scan.covering_populated {
                if let Some(cols) = &record.included_columns {
                    scan.existing_columns.clone_from(cols);
                    *scan.covering_populated = true;
                }
            }
        }
    }
}

pub mod manifest_access;
pub mod memory_management;
pub mod pending_delta;

impl IndexDataManagerImpl {
    pub fn new() -> Self {
        Self::new_with_optional_root(None)
    }

    pub fn new_with_root(index_root: impl Into<PathBuf>) -> Self {
        Self::new_with_optional_root(Some(index_root.into()))
    }

    fn new_with_optional_root(index_root: Option<PathBuf>) -> Self {
        Self {
            index_root,
            manifest_catalogs: Arc::new(RwLock::new(HashMap::new())),
            runtimes: Arc::new(RwLock::new(HashMap::new())),
            index_aliases: Arc::new(RwLock::new(HashMap::new())),
            index_types: Arc::new(RwLock::new(HashMap::new())),
            index_definitions: Arc::new(RwLock::new(HashMap::new())),
            restored_generations: Arc::new(RwLock::new(HashMap::new())),
            barrier_registry: Arc::new(RwLock::new(HashMap::new())),
            rebuild_gate: Arc::new(RwLock::new(())),
            stats_manager: None,
            memory_limit_bytes: Arc::new(AtomicU64::new(0)),
            total_memory_usage: Arc::new(AtomicU64::new(0)),
            pool_capacity: Arc::new(AtomicU64::new(128 * 1024 * 1024)),
            eviction_enabled: Arc::new(AtomicBool::new(true)),
            eviction_high_ratio: Arc::new(AtomicU64::new(8500)),
            eviction_low_ratio: Arc::new(AtomicU64::new(6500)),
            cached_tombstone_count: Arc::new(AtomicU64::new(0)),
            pending_deltas: Arc::new(Mutex::new(HashMap::new())),
            delta_publish_threshold: Arc::new(AtomicUsize::new(512)),
        }
    }
}

/// Remove `path` only when it is an empty directory (e.g. a generation
/// directory left behind after its shard checkpoints were reclaimed).
pub(crate) fn remove_dir_if_empty(path: &Path) {
    if path.is_dir() && std::fs::read_dir(path).is_ok_and(|mut it| it.next().is_none()) {
        let _ = std::fs::remove_dir(path);
    }
}

impl Default for IndexDataManagerImpl {
    fn default() -> Self {
        Self::new()
    }
}
