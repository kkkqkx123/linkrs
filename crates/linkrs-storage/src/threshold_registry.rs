//! Central registry of storage-layer tunable thresholds.
//!
//! Every tunable threshold in the storage engine is listed in [`REGISTRY`]
//! with its unit, default value, tuning surface(s), covering benchmarks,
//! and the modules affected by a retune. The gate tests below pin each
//! constant (renaming one breaks the build), assert that every documented
//! default matches the live value, and verify that linked benchmark
//! functions still exist and stay registered.
//!
//! Tuning surfaces:
//! - `config_file`: a `[storage]` key in `config.toml`, deserialized into
//!   `linkrs-config::StorageConfig` and mapped at server startup.
//! - `engine_struct`: a field on an engine-side config struct
//!   (constructed directly in code, or seeded from `StorageConfig`).
//! - `hardcoded`: a constant in this crate.
//!
//! Registry policy: a threshold either links one or more benchmark
//! functions that observe it, or carries a `note` explaining why no bench
//! can. Documented defaults are asserted equal to live values, so retuning
//! a constant or a config default without updating this table fails tests.
//! Coverage baseline is the storage threshold analysis inventory plus the
//! process-level memory watermark constants.

use crate::cache::config::RecordCacheConfig;
use crate::compression::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};
use crate::cursor::ScanOptions;
use crate::edge::edge_table::config::{AutoMaintenanceConfig, EdgeTableConfig};
use crate::edge::edge_table::core::index::INDEX_SEGMENT_CUTOVER;
use crate::edge::edge_table::iterator::DEFAULT_ADJACENCY_BATCH;
use crate::edge::edge_table::wal::MAX_ENTRY_LEN;
use crate::edge::fragmentation_stats::GROUP_FRAGMENTATION_THRESHOLD;
use crate::edge::mutable_csr::write::TOMBSTONE_REUSE_SCAN_BOUND;
use crate::edge::node_group::{
    DEFAULT_NODE_GROUP_BITS, GROUP_MERGE_MIN_DENSITY, REGION_MERGE_MIN_DENSITY,
};
use crate::edge::property_schema::DEFAULT_VERSION_CHAIN_CAP;
use crate::encoding::fsst::{
    DEFAULT_REBUILD_THRESHOLD as FSST_DEFAULT_REBUILD_THRESHOLD, MAX_NGRAMS_PER_STRING,
    MAX_TRAINING_SAMPLES,
};
use crate::encoding::selector::{EncodingFeedback, EncodingThresholds};
use crate::engine::config::{
    FreezeConfig, FlushConfig, PropertyGraphConfig, ResourceConfig,
};
use crate::engine::graph_storage::context::conflict::{
    COMMITTED_WRITE_SET_CAPACITY, MAX_PUBLISHED_WRITE_SET_SIZE,
};
use crate::engine::graph_storage::context::txn_staging::MAX_TXN_STAGING_KEYS;
use crate::engine::graph_storage::index_manager::checkpoint::POOL_CAPACITY;
use crate::engine::graph_storage::reader::edge::SCAN_PAGE;
use crate::engine::graph_storage::ttl::PURGE_DELETE_BATCH;
use crate::engine::persistence_coordinator::config::PersistenceConfig;
use crate::index::shard_runtime::bloom::SATURATION_QUERIES;
use crate::index::shard_runtime::runtime::DEFAULT_POOL_CAPACITY;
use crate::index::shard_runtime::shard::WAL_CHECKPOINT_THRESHOLD;
use crate::memory_watermark::{
    CRITICAL_MARGIN, DEFAULT_CRITICAL_PCT, DEFAULT_HIGH_PCT, DEFAULT_PROCESS_BUDGET_BYTES,
    HIGH_MARGIN,
};
use crate::persistence::dirty_page::ROWS_PER_PAGE;
use crate::vertex::column::chunk::DEFAULT_CHUNK_ROWS;
use crate::vertex::column::chunk_encoding::{
    DEFAULT_ALP_EXCEPTION_THRESHOLD, DEFAULT_DICT_MAX_ENTRIES_PER_CHUNK, OVERLAY_CAPACITY_DIVISOR,
};
use crate::vertex::column::column::{EVICTION_SEGMENT_BYTES, MAX_BACKGROUND_LOAD_CHUNKS};
use crate::vertex::column::overflow::DEFAULT_OVERFLOW_THRESHOLD;
use crate::vertex::column::zone_map::{ZONE_MAP_CHUNK_ROWS, ZONE_STALE_REBUILD_THRESHOLD};
use crate::vertex::gc_manager::STUCK_SNAPSHOT_AGE_SECS;
use crate::vertex::id_indexer::config::{
    DEFAULT_GROWTH_FACTOR, DEFAULT_INITIAL_CAPACITY, MAX_CAPACITY,
};
use crate::vertex::id_indexer::manager::ID_STRIPE_COUNT;
use crate::vertex::id_indexer::{IdManager, PK_DELTA_ANCHOR_THRESHOLD};
use crate::vertex::vertex_table::flush_trigger::{
    MAX_BASELINE_AGE_MS, MIN_BASELINE_AGE_MS, PAGE_MERGE_RATIO,
};
use crate::vertex::vertex_table::sharded::maintenance::thresholds::{
    SHARD_FRAGMENTATION_THRESHOLD, STABLE_ROW_ID_HOLE_WATERMARK,
};
use crate::vertex::vertex_table::sharded::routing::{
    DEFAULT_SEGMENT_SLOTS_BITS, SINGLE_SHARD_MAX_ESTIMATE,
};
use crate::vertex::write_scope::MAX_WRITE_SCOPE_KEYS;

/// Kind-tagged threshold value used by the registry for equality checks.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ThresholdValue {
    U64(u64),
    U32(u32),
    Usize(usize),
    F64(f64),
    F32(f32),
    Bool(bool),
    Pair(u32, u32),
}

/// Surfaces a threshold can be retuned from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TunableLayers {
    /// `config.toml [storage]` key on `linkrs-config::StorageConfig`.
    config_file: bool,
    /// Field on an engine-side config struct.
    engine_struct: bool,
    /// Hardcoded constant only.
    hardcoded: bool,
}

const LAYER_AB: TunableLayers = TunableLayers {
    config_file: true,
    engine_struct: true,
    hardcoded: false,
};
const LAYER_B: TunableLayers = TunableLayers {
    config_file: false,
    engine_struct: true,
    hardcoded: false,
};
const LAYER_BC: TunableLayers = TunableLayers {
    config_file: false,
    engine_struct: true,
    hardcoded: true,
};
const LAYER_C: TunableLayers = TunableLayers {
    config_file: false,
    engine_struct: false,
    hardcoded: true,
};

/// One registered threshold.
struct ThresholdEntry {
    /// Identifier, usually the defining constant or config field name.
    id: &'static str,
    /// Value unit: `bytes`, `rows`, `ratio`, `percent`, `seconds`,
    /// `milliseconds`, `count`, `bits`, or `bool`.
    unit: &'static str,
    /// Defining file, relative to the crate root (or `linkrs-*` prefixed
    /// for cross-crate defaults).
    source: &'static str,
    /// Tuning surfaces.
    layers: TunableLayers,
    /// Returns the live default. Referencing the constant or config
    /// default here is the compile-time pin: renames break the build.
    live_default: fn() -> ThresholdValue,
    /// Documented default; asserted equal to `live_default()`.
    documented_default: ThresholdValue,
    /// Benchmark functions that observe this threshold.
    benches: &'static [&'static str],
    /// Modules whose behavior shifts when this threshold changes.
    affected_modules: &'static [&'static str],
    /// Free-form annotation; must explain the exemption when `benches`
    /// is empty.
    note: &'static str,
}

const REGISTRY: &[ThresholdEntry] = &[
    // ---- memory budget and pressure ----
    ThresholdEntry {
        id: "ResourceConfig.max_memory_bytes",
        unit: "bytes",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::U64(ResourceConfig::default().max_memory_bytes),
        documented_default: ThresholdValue::U64(512 * 1024 * 1024),
        benches: &[],
        affected_modules: &["engine/resource_budget", "engine/graph_storage"],
        note: "instance-wide hard budget; validated together with the ratio pair below",
    },
    ThresholdEntry {
        id: "ResourceConfig.index_memory_bytes",
        unit: "bytes",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::U64(ResourceConfig::default().index_memory_bytes),
        documented_default: ThresholdValue::U64(128 * 1024 * 1024),
        benches: &[],
        affected_modules: &["engine/resource_budget", "index"],
        note: "independent index share; must stay below max_memory_bytes",
    },
    ThresholdEntry {
        id: "ResourceConfig.memory_soft_ratio",
        unit: "ratio",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::F64(ResourceConfig::default().memory_soft_ratio),
        documented_default: ThresholdValue::F64(0.80),
        benches: &[],
        affected_modules: &["engine/resource_budget"],
        note: "schedules background maintenance; must stay below memory_hard_ratio",
    },
    ThresholdEntry {
        id: "ResourceConfig.memory_hard_ratio",
        unit: "ratio",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::F64(ResourceConfig::default().memory_hard_ratio),
        documented_default: ThresholdValue::F64(0.95),
        benches: &[],
        affected_modules: &["engine/resource_budget"],
        note: "rejects new work with a capacity error at this usage ratio",
    },
    ThresholdEntry {
        id: "ResourceConfig.index_pool_capacity_bytes",
        unit: "bytes",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::U64(ResourceConfig::default().index_pool_capacity_bytes),
        documented_default: ThresholdValue::U64(128 * 1024 * 1024),
        benches: &[],
        affected_modules: &["index/shard_runtime"],
        note: "per-shard index buffer pool capacity",
    },
    ThresholdEntry {
        id: "ResourceConfig.index_eviction_high_ratio",
        unit: "ratio",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::F64(ResourceConfig::default().index_eviction_high_ratio),
        documented_default: ThresholdValue::F64(0.85),
        benches: &[],
        affected_modules: &["engine/resource_budget", "index/shard_runtime"],
        note: "eviction trigger watermark; paired with the low-ratio hysteresis target",
    },
    ThresholdEntry {
        id: "ResourceConfig.index_eviction_low_ratio",
        unit: "ratio",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::F64(ResourceConfig::default().index_eviction_low_ratio),
        documented_default: ThresholdValue::F64(0.65),
        benches: &[],
        affected_modules: &["engine/resource_budget", "index/shard_runtime"],
        note: "eviction target watermark; forms the 0.85/0.65 hysteresis pair",
    },
    // ---- process-level memory watermark ----
    ThresholdEntry {
        id: "DEFAULT_PROCESS_BUDGET_BYTES",
        unit: "bytes",
        source: "memory_watermark.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(DEFAULT_PROCESS_BUDGET_BYTES),
        documented_default: ThresholdValue::U64(2 * 1024 * 1024 * 1024),
        benches: &[],
        affected_modules: &["memory_watermark", "query admission"],
        note: "query-side process budget; storage-side eviction uses the instance pressure level instead",
    },
    ThresholdEntry {
        id: "DEFAULT_HIGH_PCT",
        unit: "percent",
        source: "memory_watermark.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(DEFAULT_HIGH_PCT),
        documented_default: ThresholdValue::U64(70),
        benches: &[],
        affected_modules: &["memory_watermark"],
        note: "stops building columnar acceleration structures at this usage",
    },
    ThresholdEntry {
        id: "DEFAULT_CRITICAL_PCT",
        unit: "percent",
        source: "memory_watermark.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(DEFAULT_CRITICAL_PCT),
        documented_default: ThresholdValue::U64(90),
        benches: &[],
        affected_modules: &["memory_watermark"],
        note: "additionally unloads optional buffers at this usage",
    },
    ThresholdEntry {
        id: "HIGH_MARGIN",
        unit: "ratio",
        source: "memory_watermark.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F64(HIGH_MARGIN),
        documented_default: ThresholdValue::F64(0.10),
        benches: &[],
        affected_modules: &["memory_watermark"],
        note: "hysteresis margin around the high watermark",
    },
    ThresholdEntry {
        id: "CRITICAL_MARGIN",
        unit: "ratio",
        source: "memory_watermark.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F64(CRITICAL_MARGIN),
        documented_default: ThresholdValue::F64(0.05),
        benches: &[],
        affected_modules: &["memory_watermark"],
        note: "hysteresis margin around the critical watermark",
    },
    // ---- cache ----
    ThresholdEntry {
        id: "PropertyGraphConfig.cache_memory",
        unit: "bytes",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::Usize(PropertyGraphConfig::default().cache_memory),
        documented_default: ThresholdValue::Usize(128 * 1024 * 1024),
        benches: &[],
        affected_modules: &["cache", "engine/config"],
        note: "record cache budget; config.toml key cache_memory_bytes",
    },
    ThresholdEntry {
        id: "ResourceConfig.cache_ttl",
        unit: "seconds",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::U64(
                ResourceConfig::default()
                    .cache_ttl
                    .map(|ttl| ttl.as_secs())
                    .unwrap_or(0),
            )
        },
        documented_default: ThresholdValue::U64(300),
        benches: &[],
        affected_modules: &["cache"],
        note: "absolute entry age cap; config.toml key cache_ttl_secs (0 disables)",
    },
    ThresholdEntry {
        id: "ResourceConfig.cache_tti",
        unit: "seconds",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::U64(
                ResourceConfig::default()
                    .cache_tti
                    .map(|tti| tti.as_secs())
                    .unwrap_or(0),
            )
        },
        documented_default: ThresholdValue::U64(60),
        benches: &[],
        affected_modules: &["cache"],
        note: "idle eviction window; must stay <= cache_ttl or the idle term never binds",
    },
    ThresholdEntry {
        id: "RecordCacheConfig.max_memory",
        unit: "bytes",
        source: "cache/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::Usize(RecordCacheConfig::default().max_memory),
        documented_default: ThresholdValue::Usize(128 * 1024 * 1024),
        benches: &[],
        affected_modules: &["cache"],
        note: "build-time cache cap; recreating the cache is required to retune",
    },
    ThresholdEntry {
        id: "RecordCacheConfig.memory_ratio",
        unit: "ratio",
        source: "cache/config.rs",
        layers: LAYER_B,
        live_default: || {
            let ratio = RecordCacheConfig::default().memory_ratio;
            ThresholdValue::Pair(ratio.0, ratio.1)
        },
        documented_default: ThresholdValue::Pair(70, 30),
        benches: &[],
        affected_modules: &["cache"],
        note: "(vertex, id_index) split of the cache budget",
    },
    ThresholdEntry {
        id: "RecordCacheConfig.high_priority_ratio",
        unit: "ratio",
        source: "cache/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::F32(RecordCacheConfig::default().high_priority_ratio),
        documented_default: ThresholdValue::F32(0.1),
        benches: &[],
        affected_modules: &["cache"],
        note: "share of cache memory reserved for high-priority id_index entries",
    },
    ThresholdEntry {
        id: "ResourceConfig.cache_eviction_sync",
        unit: "bool",
        source: "engine/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::Bool(ResourceConfig::default().cache_eviction_sync),
        documented_default: ThresholdValue::Bool(true),
        benches: &[],
        affected_modules: &["cache"],
        note: "when true every eviction immediately updates memory accounting",
    },
    // ---- flush / checkpoint / WAL ----
    ThresholdEntry {
        id: "ResourceConfig.dirty_flush_operations",
        unit: "count",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::U64(ResourceConfig::default().dirty_flush_operations),
        documented_default: ThresholdValue::U64(50_000),
        benches: &[],
        affected_modules: &["engine/persistence", "vertex"],
        note: "dirty-operation count that arms a flush request",
    },
    ThresholdEntry {
        id: "ResourceConfig.dirty_flush_bytes",
        unit: "bytes",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::U64(ResourceConfig::default().dirty_flush_bytes),
        documented_default: ThresholdValue::U64(64 * 1024 * 1024),
        benches: &[],
        affected_modules: &["engine/persistence", "vertex"],
        note: "estimated dirty bytes that arms a flush request",
    },
    ThresholdEntry {
        id: "FlushConfig.flush_interval",
        unit: "seconds",
        source: "engine/config.rs",
        layers: LAYER_B,
        live_default: || {
            ThresholdValue::U64(FlushConfig::default().flush_interval.as_secs())
        },
        documented_default: ThresholdValue::U64(30),
        benches: &[],
        affected_modules: &["engine/persistence"],
        note: "flush cadence; the operation/byte triggers live on ResourceConfig",
    },
    ThresholdEntry {
        id: "PersistenceConfig.auto_flush_interval",
        unit: "seconds",
        source: "engine/persistence/config.rs",
        layers: LAYER_B,
        live_default: || {
            ThresholdValue::U64(PersistenceConfig::default().auto_flush_interval.as_secs())
        },
        documented_default: ThresholdValue::U64(60),
        benches: &[],
        affected_modules: &["engine/persistence"],
        note: "automatic background flush cadence",
    },
    ThresholdEntry {
        id: "PersistenceConfig.auto_checkpoint_interval",
        unit: "seconds",
        source: "engine/persistence/config.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::U64(PersistenceConfig::default().auto_checkpoint_interval.as_secs())
        },
        documented_default: ThresholdValue::U64(300),
        benches: &[],
        affected_modules: &["engine/persistence"],
        note: "automatic checkpoint cadence; config.toml key checkpoint_interval_secs",
    },
    ThresholdEntry {
        id: "PersistenceConfig.checkpoint_threshold",
        unit: "count",
        source: "engine/persistence/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::U64(PersistenceConfig::default().checkpoint_threshold),
        documented_default: ThresholdValue::U64(10_000),
        benches: &["bench_real_checkpoint"],
        affected_modules: &["engine/persistence", "index/shard_runtime"],
        note: "operation count between checkpoints; scaled checkpoints exercise the rewrite path",
    },
    ThresholdEntry {
        id: "PersistenceConfig.max_wal_size",
        unit: "bytes",
        source: "engine/persistence/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::U64(PersistenceConfig::default().max_wal_size),
        documented_default: ThresholdValue::U64(100 * 1024 * 1024),
        benches: &["bench_restart"],
        affected_modules: &["engine/persistence", "linkrs-transaction"],
        note: "WAL growth cap; restart replays whatever the cap allowed to accumulate",
    },
    ThresholdEntry {
        id: "PersistenceConfig.wal_buffer_size",
        unit: "bytes",
        source: "engine/persistence/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::Usize(PersistenceConfig::default().wal_buffer_size),
        documented_default: ThresholdValue::Usize(256 * 1024),
        benches: &["bench_sync_policy_ingest"],
        affected_modules: &["engine/persistence"],
        note: "per-thread async WAL flush buffer",
    },
    ThresholdEntry {
        id: "PersistenceConfig.wal_flush_interval_ms",
        unit: "milliseconds",
        source: "engine/persistence/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::U64(PersistenceConfig::default().wal_flush_interval_ms),
        documented_default: ThresholdValue::U64(10),
        benches: &["bench_sync_policy_ingest"],
        affected_modules: &["engine/persistence"],
        note: "background WAL flush cadence",
    },
    ThresholdEntry {
        id: "CHECKPOINT_EXECUTION_TIMEOUT",
        unit: "seconds",
        source: "engine/persistence/checkpoint_scheduler.rs",
        layers: LAYER_C,
        live_default: || {
            ThresholdValue::U64(
                crate::engine::persistence_coordinator::checkpoint_scheduler::CHECKPOINT_EXECUTION_TIMEOUT
                    .as_secs(),
            )
        },
        documented_default: ThresholdValue::U64(30),
        benches: &[],
        affected_modules: &["engine/persistence"],
        note: "reserved execution timeout for the checkpoint scheduler; currently unused",
    },
    ThresholdEntry {
        id: "WAL_CHECKPOINT_THRESHOLD",
        unit: "bytes",
        source: "index/shard_runtime/shard.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(WAL_CHECKPOINT_THRESHOLD),
        documented_default: ThresholdValue::U64(1024 * 1024),
        benches: &[],
        affected_modules: &["index/shard_runtime"],
        note: "below this WAL size, flush appends entries instead of checkpointing; no bench isolates the cutoff",
    },
    ThresholdEntry {
        id: "MAX_ENTRY_LEN",
        unit: "bytes",
        source: "edge/edge_table/wal.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_ENTRY_LEN),
        documented_default: ThresholdValue::Usize(64 * 1024 * 1024),
        benches: &[],
        affected_modules: &["edge/edge_table"],
        note: "bounds one edge-WAL entry payload during scans; a correctness guard rather than a tuning knob",
    },
    ThresholdEntry {
        id: "MAX_BASELINE_AGE_MS",
        unit: "milliseconds",
        source: "vertex/vertex_table/flush_trigger.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(MAX_BASELINE_AGE_MS),
        documented_default: ThresholdValue::U64(300_000),
        benches: &["bench_scaled_checkpoint"],
        affected_modules: &["vertex/vertex_table"],
        note: "silent tables also anchor a new baseline past this age",
    },
    ThresholdEntry {
        id: "MIN_BASELINE_AGE_MS",
        unit: "milliseconds",
        source: "vertex/vertex_table/flush_trigger.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(MIN_BASELINE_AGE_MS),
        documented_default: ThresholdValue::U64(30_000),
        benches: &["bench_scaled_checkpoint"],
        affected_modules: &["vertex/vertex_table"],
        note: "minimum baseline age before skip branches may fire",
    },
    ThresholdEntry {
        id: "PAGE_MERGE_RATIO",
        unit: "ratio",
        source: "vertex/vertex_table/flush_trigger.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F64(PAGE_MERGE_RATIO),
        documented_default: ThresholdValue::F64(0.5),
        benches: &["bench_real_checkpoint"],
        affected_modules: &["vertex/vertex_table"],
        note: "dirty-page ratio that triggers page merge",
    },
    ThresholdEntry {
        id: "PK_DELTA_ANCHOR_THRESHOLD",
        unit: "rows",
        source: "vertex/id_indexer.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(PK_DELTA_ANCHOR_THRESHOLD),
        documented_default: ThresholdValue::Usize(8_192),
        benches: &["bench_vertex_churn_reuse"],
        affected_modules: &["vertex/id_indexer"],
        note: "delta anchor floor; the effective threshold is scale-aware (see anchor_threshold_for_live)",
    },
    // ---- freeze / compaction / merge ----
    ThresholdEntry {
        id: "FreezeConfig.delta_edge_threshold",
        unit: "count",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::U64(FreezeConfig::default().delta_edge_threshold),
        documented_default: ThresholdValue::U64(100_000),
        benches: &["bench_freeze_reclaim"],
        affected_modules: &["edge/mutable_csr", "edge/edge_table"],
        note: "presets: development 50K, production_small 100K, production_large 500K; default is production_small",
    },
    ThresholdEntry {
        id: "FreezeConfig.delta_memory_threshold_bytes",
        unit: "bytes",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::U64(FreezeConfig::default().delta_memory_threshold_bytes)
        },
        documented_default: ThresholdValue::U64(256 * 1024 * 1024),
        benches: &["bench_freeze_reclaim"],
        affected_modules: &["edge/mutable_csr", "edge/edge_table"],
        note: "presets: development 128MB, production_small 256MB, production_large 512MB",
    },
    ThresholdEntry {
        id: "FreezeConfig.deletion_threshold",
        unit: "ratio",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::F64(FreezeConfig::default().deletion_threshold),
        documented_default: ThresholdValue::F64(0.2),
        benches: &["bench_freeze_reclaim"],
        affected_modules: &["edge/mutable_csr", "edge/edge_table"],
        note: "presets: development 0.5, production_small 0.2, production_large 0.3; high churn reclaims without waiting for size thresholds",
    },
    ThresholdEntry {
        id: "SHARD_FRAGMENTATION_THRESHOLD",
        unit: "ratio",
        source: "vertex/vertex_table/sharded/maintenance/thresholds.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F64(SHARD_FRAGMENTATION_THRESHOLD),
        documented_default: ThresholdValue::F64(0.25),
        benches: &["bench_scan_all_vertices"],
        affected_modules: &["vertex/vertex_table/sharded"],
        note: "above this hole rate a shard becomes a selective-compaction candidate",
    },
    ThresholdEntry {
        id: "STABLE_ROW_ID_HOLE_WATERMARK",
        unit: "ratio",
        source: "vertex/vertex_table/sharded/maintenance/thresholds.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F64(STABLE_ROW_ID_HOLE_WATERMARK),
        documented_default: ThresholdValue::F64(0.25),
        benches: &["bench_scan_all_vertices"],
        affected_modules: &["vertex/vertex_table/sharded"],
        note: "alias of SHARD_FRAGMENTATION_THRESHOLD; one named anchor for the hole-rate policy",
    },
    ThresholdEntry {
        id: "GROUP_FRAGMENTATION_THRESHOLD",
        unit: "ratio",
        source: "edge/fragmentation_stats.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F32(GROUP_FRAGMENTATION_THRESHOLD),
        documented_default: ThresholdValue::F32(0.5),
        benches: &[],
        affected_modules: &["edge"],
        note: "group-level fragmentation trigger; no bench isolates the group compaction decision",
    },
    ThresholdEntry {
        id: "REGION_MERGE_MIN_DENSITY",
        unit: "ratio",
        source: "edge/node_group/compaction.rs",
        layers: LAYER_BC,
        live_default: || ThresholdValue::F32(REGION_MERGE_MIN_DENSITY),
        documented_default: ThresholdValue::F32(0.4),
        benches: &[],
        affected_modules: &["edge/node_group", "edge/edge_table"],
        note: "EdgeTableConfig.region_merge_min_density mirrors this constant at construction",
    },
    ThresholdEntry {
        id: "GROUP_MERGE_MIN_DENSITY",
        unit: "ratio",
        source: "edge/node_group/compaction.rs",
        layers: LAYER_BC,
        live_default: || ThresholdValue::F32(GROUP_MERGE_MIN_DENSITY),
        documented_default: ThresholdValue::F32(0.65),
        benches: &[],
        affected_modules: &["edge/node_group", "edge/edge_table"],
        note: "EdgeTableConfig.group_merge_min_density mirrors this constant at construction",
    },
    ThresholdEntry {
        id: "AutoCompactConfig.min_holes",
        unit: "count",
        source: "linkrs-core/src/types/compact.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::U64(u64::from(
                linkrs_core::types::AutoCompactConfig::default().min_holes,
            ))
        },
        documented_default: ThresholdValue::U64(65_536),
        benches: &[],
        affected_modules: &["vertex/vertex_table", "maintenance_scheduler"],
        note: "minimum id holes before automatic vertex compaction runs; config.toml key auto_compact_min_holes",
    },
    ThresholdEntry {
        id: "AutoCompactConfig.min_hole_ratio",
        unit: "ratio",
        source: "linkrs-core/src/types/compact.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::F32(linkrs_core::types::AutoCompactConfig::default().min_hole_ratio)
        },
        documented_default: ThresholdValue::F32(0.25),
        benches: &[],
        affected_modules: &["vertex/vertex_table", "maintenance_scheduler"],
        note: "hole ratio paired with the absolute hole count; config.toml key auto_compact_min_hole_ratio",
    },
    ThresholdEntry {
        id: "AutoCompactConfig.min_interval_secs",
        unit: "seconds",
        source: "linkrs-core/src/types/compact.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::U64(
                linkrs_core::types::AutoCompactConfig::default().min_interval_secs,
            )
        },
        documented_default: ThresholdValue::U64(3_600),
        benches: &[],
        affected_modules: &["vertex/vertex_table", "maintenance_scheduler"],
        note: "compaction cooldown; config.toml key auto_compact_min_interval_secs",
    },
    ThresholdEntry {
        id: "EdgeTableConfig.max_mutable_csr_bytes",
        unit: "bytes",
        source: "edge/edge_table/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::Usize(EdgeTableConfig::default().max_mutable_csr_bytes),
        documented_default: ThresholdValue::Usize(100 * 1024 * 1024),
        benches: &[],
        affected_modules: &["edge/mutable_csr", "edge/edge_table"],
        note: "per-group CSR write backpressure; config.toml key edge_max_mutable_csr_bytes, zero disables",
    },
    ThresholdEntry {
        id: "EdgeTableConfig.max_append_ops_per_group",
        unit: "count",
        source: "edge/edge_table/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::Usize(EdgeTableConfig::default().max_append_ops_per_group),
        documented_default: ThresholdValue::Usize(4_096),
        benches: &[],
        affected_modules: &["edge/edge_table"],
        note: "sidecar op count that forces a base rewrite at the next checkpoint",
    },
    ThresholdEntry {
        id: "ResourceConfig.background_freeze_min_interval",
        unit: "milliseconds",
        source: "engine/config.rs",
        layers: LAYER_B,
        live_default: || {
            ThresholdValue::U64(
                ResourceConfig::default()
                    .background_freeze_min_interval
                    .as_millis() as u64,
            )
        },
        documented_default: ThresholdValue::U64(250),
        benches: &[],
        affected_modules: &["edge/edge_table", "maintenance_scheduler"],
        note: "write-path re-trigger cooldown that breaks the background-freeze feedback loop",
    },
    // ---- snapshot / GC / tombstone ----
    ThresholdEntry {
        id: "ResourceConfig.max_active_snapshots",
        unit: "count",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::Usize(ResourceConfig::default().max_active_snapshots),
        documented_default: ThresholdValue::Usize(1_000),
        benches: &[],
        affected_modules: &["engine/snapshot_manager"],
        note: "cap on concurrent snapshot registrations",
    },
    ThresholdEntry {
        id: "ResourceConfig.max_snapshot_age",
        unit: "seconds",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::U64(ResourceConfig::default().max_snapshot_age.as_secs())
        },
        documented_default: ThresholdValue::U64(300),
        benches: &[],
        affected_modules: &["engine/snapshot_manager"],
        note: "age after which the owner rejects a snapshot",
    },
    ThresholdEntry {
        id: "ResourceConfig.max_tombstones",
        unit: "count",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::Usize(ResourceConfig::default().max_tombstones),
        documented_default: ThresholdValue::Usize(1_000_000),
        benches: &[],
        affected_modules: &["engine/gc_coordinator", "mvcc"],
        note: "MVCC tombstone count that forces backpressure",
    },
    ThresholdEntry {
        id: "ResourceConfig.max_tombstone_bytes",
        unit: "bytes",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::U64(ResourceConfig::default().max_tombstone_bytes),
        documented_default: ThresholdValue::U64(256 * 1024 * 1024),
        benches: &[],
        affected_modules: &["engine/gc_coordinator", "mvcc"],
        note: "estimated tombstone memory that forces backpressure",
    },
    ThresholdEntry {
        id: "ResourceConfig.index_gc_batch",
        unit: "count",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::Usize(ResourceConfig::default().index_gc_batch),
        documented_default: ThresholdValue::Usize(10_000),
        benches: &[],
        affected_modules: &["index/index_gc_manager"],
        note: "index entries processed by one incremental GC pass",
    },
    ThresholdEntry {
        id: "STUCK_SNAPSHOT_AGE_SECS",
        unit: "seconds",
        source: "vertex/gc_manager.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(STUCK_SNAPSHOT_AGE_SECS),
        documented_default: ThresholdValue::U64(30),
        benches: &["bench_vertex_churn_reuse"],
        affected_modules: &["vertex/gc_manager"],
        note: "warns only; stuck snapshots are never reclaimed automatically",
    },
    ThresholdEntry {
        id: "PropertyGraphConfig.gc_safety_margin",
        unit: "count",
        source: "engine/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::U64(PropertyGraphConfig::default().gc_safety_margin),
        documented_default: ThresholdValue::U64(1),
        benches: &[],
        affected_modules: &["engine/gc_coordinator"],
        note: "Timestamp units subtracted from the GC watermark; higher keeps more history",
    },
    ThresholdEntry {
        id: "AutoMaintenanceConfig.property_compact_ratio",
        unit: "ratio",
        source: "edge/edge_table/config.rs",
        layers: LAYER_B,
        live_default: || {
            ThresholdValue::F32(AutoMaintenanceConfig::default().property_compact_ratio)
        },
        documented_default: ThresholdValue::F32(0.15),
        benches: &[],
        affected_modules: &["edge/edge_table"],
        note: "deleted-but-not-reclaimed property rows that arm write-path compaction",
    },
    ThresholdEntry {
        id: "AutoMaintenanceConfig.reclaim_tombstone_threshold",
        unit: "count",
        source: "edge/edge_table/config.rs",
        layers: LAYER_B,
        live_default: || {
            ThresholdValue::Usize(AutoMaintenanceConfig::default().reclaim_tombstone_threshold)
        },
        documented_default: ThresholdValue::Usize(4),
        benches: &[],
        affected_modules: &["edge/edge_table"],
        note: "minimum tracked tombstones arming the reclaim pass when the watermark stalls",
    },
    ThresholdEntry {
        id: "AutoMaintenanceConfig.index_rebuild_failure_threshold",
        unit: "count",
        source: "edge/edge_table/config.rs",
        layers: LAYER_B,
        live_default: || {
            ThresholdValue::U64(
                AutoMaintenanceConfig::default().index_rebuild_failure_threshold,
            )
        },
        documented_default: ThresholdValue::U64(64),
        benches: &[],
        affected_modules: &["edge/edge_table"],
        note: "failure count that triggers a secondary property index rebuild",
    },
    ThresholdEntry {
        id: "AutoMaintenanceConfig.index_max_stale_secs",
        unit: "seconds",
        source: "edge/edge_table/config.rs",
        layers: LAYER_B,
        live_default: || {
            ThresholdValue::U64(AutoMaintenanceConfig::default().index_max_stale_secs)
        },
        documented_default: ThresholdValue::U64(300),
        benches: &[],
        affected_modules: &["edge/edge_table"],
        note: "age trigger for the secondary property index rebuild",
    },
    ThresholdEntry {
        id: "PURGE_DELETE_BATCH",
        unit: "rows",
        source: "engine/graph_storage/ttl.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(PURGE_DELETE_BATCH),
        documented_default: ThresholdValue::Usize(1_024),
        benches: &[],
        affected_modules: &["engine/graph_storage"],
        note: "bounds delete-loop memory; the purge cadence itself is the ttl_purge_interval_secs config key",
    },
    // ---- columnar / encoding ----
    ThresholdEntry {
        id: "DEFAULT_CHUNK_ROWS",
        unit: "rows",
        source: "vertex/column/chunk.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(DEFAULT_CHUNK_ROWS),
        documented_default: ThresholdValue::Usize(4_096),
        benches: &["bench_bulk_vertex_insert"],
        affected_modules: &["vertex/column"],
        note: "column chunk height; bulk inserts cross many chunks",
    },
    ThresholdEntry {
        id: "ROWS_PER_PAGE",
        unit: "rows",
        source: "persistence/dirty_page.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(ROWS_PER_PAGE),
        documented_default: ThresholdValue::Usize(1_024),
        benches: &[],
        affected_modules: &["engine/persistence"],
        note: "dirty-page row group size; tracked with the flush trigger metrics",
    },
    ThresholdEntry {
        id: "ZONE_MAP_CHUNK_ROWS",
        unit: "rows",
        source: "vertex/column/zone_map.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(ZONE_MAP_CHUNK_ROWS),
        documented_default: ThresholdValue::Usize(1_024),
        benches: &["bench_real_cursor_scan"],
        affected_modules: &["vertex/column"],
        note: "zone granularity for min/max pruning during scans",
    },
    ThresholdEntry {
        id: "ZONE_STALE_REBUILD_THRESHOLD",
        unit: "count",
        source: "vertex/column/zone_map.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(ZONE_STALE_REBUILD_THRESHOLD),
        documented_default: ThresholdValue::U64(1_024),
        benches: &[],
        affected_modules: &["vertex/column"],
        note: "stale zone count that triggers a zone-map rebuild",
    },
    ThresholdEntry {
        id: "DEFAULT_OVERFLOW_THRESHOLD",
        unit: "rows",
        source: "vertex/column/overflow.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(DEFAULT_OVERFLOW_THRESHOLD),
        documented_default: ThresholdValue::Usize(1_024),
        benches: &[],
        affected_modules: &["vertex/column"],
        note: "overflow list capacity before spilling",
    },
    ThresholdEntry {
        id: "EVICTION_SEGMENT_BYTES",
        unit: "bytes",
        source: "vertex/column/column.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(EVICTION_SEGMENT_BYTES),
        documented_default: ThresholdValue::U64(16 * 1024 * 1024),
        benches: &["bench_scaled_cursor_scan"],
        affected_modules: &["vertex/column"],
        note: "chunk-level eviction segment size under memory pressure",
    },
    ThresholdEntry {
        id: "MAX_BACKGROUND_LOAD_CHUNKS",
        unit: "count",
        source: "vertex/column/column.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_BACKGROUND_LOAD_CHUNKS),
        documented_default: ThresholdValue::Usize(64),
        benches: &["bench_scaled_cursor_scan"],
        affected_modules: &["vertex/column"],
        note: "chunks the background loader may keep in flight",
    },
    ThresholdEntry {
        id: "OVERLAY_CAPACITY_DIVISOR",
        unit: "count",
        source: "vertex/column/chunk_encoding.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(OVERLAY_CAPACITY_DIVISOR),
        documented_default: ThresholdValue::Usize(64),
        benches: &[],
        affected_modules: &["vertex/column"],
        note: "pending-update overlay memory is rows / divisor; no bench isolates overlay pressure",
    },
    ThresholdEntry {
        id: "DEFAULT_DICT_MAX_ENTRIES_PER_CHUNK",
        unit: "count",
        source: "vertex/column/chunk_encoding.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(DEFAULT_DICT_MAX_ENTRIES_PER_CHUNK),
        documented_default: ThresholdValue::Usize(65_536),
        benches: &[],
        affected_modules: &["vertex/column", "encoding"],
        note: "hardcoded dictionary cap; mirrors EncodingThresholds.dict_max_entries_per_chunk",
    },
    ThresholdEntry {
        id: "DEFAULT_ALP_EXCEPTION_THRESHOLD",
        unit: "ratio",
        source: "vertex/column/chunk_encoding.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F64(DEFAULT_ALP_EXCEPTION_THRESHOLD),
        documented_default: ThresholdValue::F64(0.25),
        benches: &[],
        affected_modules: &["vertex/column", "encoding"],
        note: "hardcoded ALP exception ceiling; mirrors EncodingThresholds.alp_exception_threshold",
    },
    ThresholdEntry {
        id: "DEFAULT_PAGE_SIZE",
        unit: "bytes",
        source: "compression.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(DEFAULT_PAGE_SIZE),
        documented_default: ThresholdValue::Usize(64 * 1024 - 1),
        benches: &[],
        affected_modules: &["compression"],
        note: "default compressed page payload cap",
    },
    ThresholdEntry {
        id: "MAX_PAGE_SIZE",
        unit: "bytes",
        source: "compression.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_PAGE_SIZE),
        documented_default: ThresholdValue::Usize(64 * 1024 * 1024),
        benches: &[],
        affected_modules: &["compression"],
        note: "hard upper bound for one compressed page",
    },
    ThresholdEntry {
        id: "EncodingFeedback::MAX_OBSERVATIONS",
        unit: "count",
        source: "encoding/selector.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(EncodingFeedback::MAX_OBSERVATIONS),
        documented_default: ThresholdValue::Usize(100),
        benches: &[],
        affected_modules: &["encoding"],
        note: "feedback window size for encoding re-evaluation; no bench isolates selection churn",
    },
    ThresholdEntry {
        id: "EncodingFeedback::REEVALUATE_AFTER",
        unit: "count",
        source: "encoding/selector.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(EncodingFeedback::REEVALUATE_AFTER),
        documented_default: ThresholdValue::Usize(20),
        benches: &[],
        affected_modules: &["encoding"],
        note: "observation count between encoding re-evaluations",
    },
    ThresholdEntry {
        id: "EncodingFeedback::EARLY_OBSERVATIONS",
        unit: "count",
        source: "encoding/selector.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(EncodingFeedback::EARLY_OBSERVATIONS),
        documented_default: ThresholdValue::Usize(3),
        benches: &[],
        affected_modules: &["encoding"],
        note: "minimum observations before the early re-encode check may fire",
    },
    ThresholdEntry {
        id: "EncodingFeedback::EARLY_RATIO_MARGIN",
        unit: "ratio",
        source: "encoding/selector.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F64(EncodingFeedback::EARLY_RATIO_MARGIN),
        documented_default: ThresholdValue::F64(0.2),
        benches: &[],
        affected_modules: &["encoding"],
        note: "margin above reencode_threshold that arms the early re-encode check",
    },
    ThresholdEntry {
        id: "fsst::MAX_TRAINING_SAMPLES",
        unit: "count",
        source: "encoding/fsst.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_TRAINING_SAMPLES),
        documented_default: ThresholdValue::Usize(10_000),
        benches: &[],
        affected_modules: &["encoding/fsst"],
        note: "sample cap for symbol-table training",
    },
    ThresholdEntry {
        id: "fsst::MAX_NGRAMS_PER_STRING",
        unit: "count",
        source: "encoding/fsst.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_NGRAMS_PER_STRING),
        documented_default: ThresholdValue::Usize(1_000),
        benches: &[],
        affected_modules: &["encoding/fsst"],
        note: "per-string n-gram budget during training",
    },
    ThresholdEntry {
        id: "fsst::DEFAULT_REBUILD_THRESHOLD",
        unit: "ratio",
        source: "encoding/fsst.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::F64(FSST_DEFAULT_REBUILD_THRESHOLD),
        documented_default: ThresholdValue::F64(0.2),
        benches: &[],
        affected_modules: &["encoding/fsst"],
        note: "fallback rebuild ratio when no configured threshold is passed; EncodingThresholds.fsst_rebuild_threshold carries the configurable face",
    },
    ThresholdEntry {
        id: "EncodingThresholds.string_min_rows",
        unit: "rows",
        source: "encoding/selector.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::Usize(EncodingThresholds::default().string_min_rows),
        documented_default: ThresholdValue::Usize(50),
        benches: &[],
        affected_modules: &["encoding"],
        note: "config.toml key string_min_rows exists on StorageConfig but startup does not map it into EncodingThresholds yet",
    },
    ThresholdEntry {
        id: "EncodingThresholds.avg_length_threshold",
        unit: "bytes",
        source: "encoding/selector.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::Usize(EncodingThresholds::default().avg_length_threshold),
        documented_default: ThresholdValue::Usize(16),
        benches: &[],
        affected_modules: &["encoding"],
        note: "config.toml key avg_length_threshold exists on StorageConfig but startup does not map it into EncodingThresholds yet",
    },
    ThresholdEntry {
        id: "EncodingThresholds.cardinality_ratio_threshold",
        unit: "ratio",
        source: "encoding/selector.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::F64(EncodingThresholds::default().cardinality_ratio_threshold)
        },
        documented_default: ThresholdValue::F64(0.5),
        benches: &[],
        affected_modules: &["encoding"],
        note: "config.toml key cardinality_ratio_threshold exists on StorageConfig but startup does not map it into EncodingThresholds yet",
    },
    ThresholdEntry {
        id: "EncodingThresholds.fsst_rebuild_threshold",
        unit: "ratio",
        source: "encoding/selector.rs",
        layers: LAYER_AB,
        live_default: || ThresholdValue::F64(EncodingThresholds::default().fsst_rebuild_threshold),
        documented_default: ThresholdValue::F64(0.2),
        benches: &[],
        affected_modules: &["encoding"],
        note: "config.toml key fsst_rebuild_threshold exists on StorageConfig but startup does not map it into EncodingThresholds yet",
    },
    ThresholdEntry {
        id: "EncodingThresholds.reencode_threshold",
        unit: "ratio",
        source: "encoding/selector.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::F64(EncodingThresholds::default().reencode_threshold),
        documented_default: ThresholdValue::F64(0.8),
        benches: &[],
        affected_modules: &["encoding"],
        note: "compressed/raw ratio that recommends re-encoding; no config.toml key yet",
    },
    ThresholdEntry {
        id: "EncodingThresholds.fsst_max_symbols",
        unit: "count",
        source: "encoding/selector.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::Usize(EncodingThresholds::default().fsst_max_symbols),
        documented_default: ThresholdValue::Usize(255),
        benches: &[],
        affected_modules: &["encoding/fsst"],
        note: "hard FSST symbol-table cap lives in fsst::SYMBOL_TABLE_SIZE; this face feeds training",
    },
    ThresholdEntry {
        id: "EncodingThresholds.alp_exception_threshold",
        unit: "ratio",
        source: "encoding/selector.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::F64(EncodingThresholds::default().alp_exception_threshold),
        documented_default: ThresholdValue::F64(0.25),
        benches: &[],
        affected_modules: &["encoding"],
        note: "mirrors DEFAULT_ALP_EXCEPTION_THRESHOLD; no config.toml key yet",
    },
    ThresholdEntry {
        id: "EncodingThresholds.dict_max_entries_per_chunk",
        unit: "count",
        source: "encoding/selector.rs",
        layers: LAYER_B,
        live_default: || {
            ThresholdValue::Usize(EncodingThresholds::default().dict_max_entries_per_chunk)
        },
        documented_default: ThresholdValue::Usize(65_536),
        benches: &[],
        affected_modules: &["encoding"],
        note: "mirrors DEFAULT_DICT_MAX_ENTRIES_PER_CHUNK; no config.toml key yet",
    },
    // ---- transaction / write path ----
    ThresholdEntry {
        id: "MAX_WRITE_SCOPE_KEYS",
        unit: "count",
        source: "vertex/write_scope.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_WRITE_SCOPE_KEYS),
        documented_default: ThresholdValue::Usize(4_096),
        benches: &["bench_write_transaction_workflow"],
        affected_modules: &["vertex", "engine/transaction"],
        note: "rows one write scope may stage",
    },
    ThresholdEntry {
        id: "MAX_TXN_STAGING_KEYS",
        unit: "count",
        source: "engine/graph_storage/context/txn_staging.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_TXN_STAGING_KEYS),
        documented_default: ThresholdValue::Usize(1 << 20),
        benches: &[],
        affected_modules: &["engine/graph_storage"],
        note: "staged keys per transaction before rejection; no bench isolates staging pressure",
    },
    ThresholdEntry {
        id: "COMMITTED_WRITE_SET_CAPACITY",
        unit: "count",
        source: "engine/graph_storage/context/conflict.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(COMMITTED_WRITE_SET_CAPACITY),
        documented_default: ThresholdValue::Usize(512),
        benches: &[],
        affected_modules: &["engine/graph_storage"],
        note: "certification window size; commit_gate_bench scenarios cover commit scaling but define no bench_ entry point",
    },
    ThresholdEntry {
        id: "MAX_PUBLISHED_WRITE_SET_SIZE",
        unit: "count",
        source: "engine/graph_storage/context/conflict.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_PUBLISHED_WRITE_SET_SIZE),
        documented_default: ThresholdValue::Usize(8_192),
        benches: &[],
        affected_modules: &["engine/graph_storage"],
        note: "bulk statements above this size skip publishing to the certification window",
    },
    ThresholdEntry {
        id: "TOMBSTONE_REUSE_SCAN_BOUND",
        unit: "count",
        source: "edge/mutable_csr/write.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(TOMBSTONE_REUSE_SCAN_BOUND),
        documented_default: ThresholdValue::Usize(64),
        benches: &["bench_tombstone_dense_insert"],
        affected_modules: &["edge/mutable_csr"],
        note: "scan bound for tombstone-slot reuse; the dense-insert bench exercises the reuse path",
    },
    // ---- index ----
    ThresholdEntry {
        id: "DEFAULT_INITIAL_CAPACITY",
        unit: "count",
        source: "vertex/id_indexer/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::Usize(DEFAULT_INITIAL_CAPACITY),
        documented_default: ThresholdValue::Usize(1_024),
        benches: &["bench_sparse_id_insert_throughput"],
        affected_modules: &["vertex/id_indexer"],
        note: "IdIndexerConfig initial stripe capacity",
    },
    ThresholdEntry {
        id: "DEFAULT_GROWTH_FACTOR",
        unit: "ratio",
        source: "vertex/id_indexer/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::F64(DEFAULT_GROWTH_FACTOR),
        documented_default: ThresholdValue::F64(1.5),
        benches: &[],
        affected_modules: &["vertex/id_indexer"],
        note: "IdIndexerConfig growth factor on resize",
    },
    ThresholdEntry {
        id: "MAX_CAPACITY",
        unit: "count",
        source: "vertex/id_indexer/config.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::Usize(MAX_CAPACITY),
        documented_default: ThresholdValue::Usize(u32::MAX as usize),
        benches: &[],
        affected_modules: &["vertex/id_indexer"],
        note: "IdIndexerConfig capacity ceiling from the u32 id space",
    },
    ThresholdEntry {
        id: "ID_STRIPE_COUNT",
        unit: "count",
        source: "vertex/id_indexer/manager.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(ID_STRIPE_COUNT),
        documented_default: ThresholdValue::Usize(16),
        benches: &["bench_sparse_id_insert_throughput"],
        affected_modules: &["vertex/id_indexer"],
        note: "hash stripes the id index is sharded across",
    },
    ThresholdEntry {
        id: "DEFAULT_POOL_CAPACITY",
        unit: "bytes",
        source: "index/shard_runtime/runtime.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(DEFAULT_POOL_CAPACITY),
        documented_default: ThresholdValue::U64(64 * 1024 * 1024),
        benches: &[],
        affected_modules: &["index/shard_runtime"],
        note: "index runtime buffer pool default capacity",
    },
    ThresholdEntry {
        id: "POOL_CAPACITY",
        unit: "bytes",
        source: "engine/graph_storage/index_manager/checkpoint.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(POOL_CAPACITY),
        documented_default: ThresholdValue::U64(64 * 1024 * 1024),
        benches: &["bench_real_checkpoint"],
        affected_modules: &["engine/graph_storage", "index"],
        note: "buffer pool capacity on the generation-checkpoint path",
    },
    ThresholdEntry {
        id: "SATURATION_QUERIES",
        unit: "count",
        source: "index/shard_runtime/bloom.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(SATURATION_QUERIES),
        documented_default: ThresholdValue::U64(1_024),
        benches: &[],
        affected_modules: &["index/shard_runtime"],
        note: "queries after which the bloom filter is treated as saturated",
    },
    ThresholdEntry {
        id: "INDEX_SEGMENT_CUTOVER",
        unit: "rows",
        source: "edge/edge_table/core/index.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(INDEX_SEGMENT_CUTOVER),
        documented_default: ThresholdValue::Usize(8_192),
        benches: &["bench_real_edge_insert"],
        affected_modules: &["edge/edge_table"],
        note: "edge-index segment cutover size; inserts cross segments over time",
    },
    ThresholdEntry {
        id: "DEFAULT_VERSION_CHAIN_CAP",
        unit: "count",
        source: "edge/property_schema.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(DEFAULT_VERSION_CHAIN_CAP),
        documented_default: ThresholdValue::Usize(64),
        benches: &[],
        affected_modules: &["edge"],
        note: "version chain length cap for edge properties",
    },
    // ---- batch / capacity ----
    ThresholdEntry {
        id: "ScanOptions::DEFAULT_BATCH_SIZE",
        unit: "rows",
        source: "cursor.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(ScanOptions::DEFAULT_BATCH_SIZE),
        documented_default: ThresholdValue::Usize(1_024),
        benches: &["bench_real_cursor_scan"],
        affected_modules: &["cursor", "engine/graph_storage"],
        note: "default cursor batch height",
    },
    ThresholdEntry {
        id: "DEFAULT_ADJACENCY_BATCH",
        unit: "rows",
        source: "edge/edge_table/iterator.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(DEFAULT_ADJACENCY_BATCH),
        documented_default: ThresholdValue::Usize(256),
        benches: &["bench_edge_read"],
        affected_modules: &["edge/edge_table"],
        note: "adjacency iteration batch size",
    },
    ThresholdEntry {
        id: "SCAN_PAGE",
        unit: "rows",
        source: "engine/graph_storage/reader/edge.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(SCAN_PAGE),
        documented_default: ThresholdValue::Usize(1_024),
        benches: &["bench_real_cursor_scan"],
        affected_modules: &["engine/graph_storage"],
        note: "internal page size for full edge scans",
    },
    ThresholdEntry {
        id: "DEFAULT_SEGMENT_SLOTS_BITS",
        unit: "bits",
        source: "vertex/vertex_table/sharded/routing.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U32(DEFAULT_SEGMENT_SLOTS_BITS),
        documented_default: ThresholdValue::U32(14),
        benches: &[],
        affected_modules: &["vertex/vertex_table/sharded"],
        note: "layout default for new tables; opened tables use the width pinned in their manifest",
    },
    ThresholdEntry {
        id: "SINGLE_SHARD_MAX_ESTIMATE",
        unit: "count",
        source: "vertex/vertex_table/sharded/routing.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::U64(SINGLE_SHARD_MAX_ESTIMATE),
        documented_default: ThresholdValue::U64(100_000),
        benches: &[],
        affected_modules: &["vertex/vertex_table/sharded"],
        note: "estimate above which shard planning stops preferring a single shard",
    },
    ThresholdEntry {
        id: "ResourceConfig.operation_timeout",
        unit: "seconds",
        source: "engine/config.rs",
        layers: LAYER_AB,
        live_default: || {
            ThresholdValue::U64(ResourceConfig::default().operation_timeout.as_secs())
        },
        documented_default: ThresholdValue::U64(30),
        benches: &[],
        affected_modules: &["engine/graph_storage", "maintenance_scheduler"],
        note: "per-maintenance-operation deadline",
    },
    ThresholdEntry {
        id: "DEFAULT_NODE_GROUP_BITS",
        unit: "bits",
        source: "edge/node_group/address.rs",
        layers: LAYER_B,
        live_default: || ThresholdValue::U32(DEFAULT_NODE_GROUP_BITS),
        documented_default: ThresholdValue::U32(12),
        benches: &[],
        affected_modules: &["edge/node_group", "edge/edge_table"],
        note: "topology node-group width locked at table creation",
    },
];

/// Locate a registry entry by id.
fn entry(id: &str) -> &'static ThresholdEntry {
    for entry in REGISTRY {
        if entry.id == id {
            return entry;
        }
    }
    panic!("threshold {id} missing from the registry")
}

/// Benchmark sources pinning the linked bench functions.
#[derive(Debug, Clone, Copy)]
enum BenchFile {
    StorageBench,
    CsrPerfBench,
    WalBench,
    CrashRecoveryBench,
    StorageWorkflowBench,
    EdgeReadAllocBench,
}

const ALL_BENCH_FILES: &[BenchFile] = &[
    BenchFile::StorageBench,
    BenchFile::CsrPerfBench,
    BenchFile::WalBench,
    BenchFile::CrashRecoveryBench,
    BenchFile::StorageWorkflowBench,
    BenchFile::EdgeReadAllocBench,
];

impl BenchFile {
    fn all() -> &'static [BenchFile] {
        ALL_BENCH_FILES
    }

    fn source(self) -> &'static str {
        match self {
            BenchFile::StorageBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/storage_bench.rs"
            )),
            BenchFile::CsrPerfBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/csr_perf_bench.rs"
            )),
            BenchFile::WalBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/wal_bench.rs"
            )),
            BenchFile::CrashRecoveryBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/crash_recovery_bench.rs"
            )),
            BenchFile::StorageWorkflowBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/storage_workflow_bench.rs"
            )),
            BenchFile::EdgeReadAllocBench => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/benches/edge_read_alloc_bench.rs"
            )),
        }
    }

    /// Criterion benches register in `criterion_group!`; plain-main benches
    /// must be called from `fn main(`.
    fn is_criterion(self) -> bool {
        !matches!(self, BenchFile::CsrPerfBench)
    }

    /// Region of the source that must mention a bench function for the
    /// function to actually run.
    fn registration_block(self) -> &'static str {
        let source = self.source();
        if self.is_criterion() {
            let start = source
                .find("criterion_group!(")
                .expect("criterion bench file holds a criterion group");
            let end = source
                .find("criterion_main!")
                .expect("criterion bench file holds a criterion main");
            &source[start..end]
        } else {
            let start = source
                .find("fn main(")
                .expect("plain bench file holds a main function");
            &source[start..]
        }
    }
}

fn find_bench_file(bench_fn: &str) -> Option<BenchFile> {
    let definition = format!("fn {bench_fn}(");
    BenchFile::all()
        .iter()
        .copied()
        .find(|file| file.source().contains(&definition))
}

#[test]
fn test_documented_defaults_match_live_values() {
    for entry in REGISTRY {
        assert_eq!(
            (entry.live_default)(),
            entry.documented_default,
            "documented default for {} drifted from the live value",
            entry.id,
        );
    }
}

#[test]
fn test_linked_benches_exist_and_stay_registered() {
    for entry in REGISTRY {
        if entry.benches.is_empty() {
            assert!(
                !entry.note.is_empty(),
                "threshold {} has no bench and needs an exemption note",
                entry.id,
            );
            continue;
        }
        for bench_fn in entry.benches {
            let file = find_bench_file(bench_fn).unwrap_or_else(|| {
                panic!(
                    "threshold {} references missing bench function {bench_fn}",
                    entry.id
                )
            });
            assert!(
                file.registration_block().contains(bench_fn),
                "threshold {} bench {bench_fn} is defined but not registered",
                entry.id,
            );
        }
    }
}

#[test]
fn test_registry_ids_are_unique_and_entries_well_formed() {
    let mut ids: Vec<&str> = REGISTRY.iter().map(|entry| entry.id).collect();
    ids.sort_unstable();
    let count = ids.len();
    ids.dedup();
    assert_eq!(count, ids.len(), "threshold ids must be unique: {ids:?}");
    for entry in REGISTRY {
        assert!(!entry.unit.is_empty(), "{} needs a unit", entry.id);
        assert!(!entry.source.is_empty(), "{} needs a source", entry.id);
        assert!(
            entry.layers.config_file || entry.layers.engine_struct || entry.layers.hardcoded,
            "{} must declare at least one tuning surface",
            entry.id,
        );
        assert!(
            !entry.affected_modules.is_empty(),
            "{} must list affected modules",
            entry.id,
        );
    }
}

#[test]
fn test_registry_pins_constants() {
    // Explicit existence pins in addition to the `live_default` closures,
    // so the pinned surface stays greppable.
    let _ = COMMITTED_WRITE_SET_CAPACITY;
    let _ = CRITICAL_MARGIN;
    let _ = DEFAULT_ADJACENCY_BATCH;
    let _ = DEFAULT_ALP_EXCEPTION_THRESHOLD;
    let _ = ScanOptions::DEFAULT_BATCH_SIZE;
    let _ = DEFAULT_CHUNK_ROWS;
    let _ = DEFAULT_CRITICAL_PCT;
    let _ = DEFAULT_DICT_MAX_ENTRIES_PER_CHUNK;
    let _ = DEFAULT_GROWTH_FACTOR;
    let _ = DEFAULT_HIGH_PCT;
    let _ = DEFAULT_INITIAL_CAPACITY;
    let _ = DEFAULT_NODE_GROUP_BITS;
    let _ = DEFAULT_OVERFLOW_THRESHOLD;
    let _ = DEFAULT_PAGE_SIZE;
    let _ = DEFAULT_POOL_CAPACITY;
    let _ = DEFAULT_PROCESS_BUDGET_BYTES;
    let _ = FSST_DEFAULT_REBUILD_THRESHOLD;
    let _ = DEFAULT_SEGMENT_SLOTS_BITS;
    let _ = DEFAULT_VERSION_CHAIN_CAP;
    let _ = EVICTION_SEGMENT_BYTES;
    let _ = GROUP_FRAGMENTATION_THRESHOLD;
    let _ = GROUP_MERGE_MIN_DENSITY;
    let _ = HIGH_MARGIN;
    let _ = ID_STRIPE_COUNT;
    let _ = INDEX_SEGMENT_CUTOVER;
    let _ = MAX_BACKGROUND_LOAD_CHUNKS;
    let _ = MAX_BASELINE_AGE_MS;
    let _ = MAX_CAPACITY;
    let _ = MAX_ENTRY_LEN;
    let _ = MAX_NGRAMS_PER_STRING;
    let _ = MAX_PAGE_SIZE;
    let _ = MAX_PUBLISHED_WRITE_SET_SIZE;
    let _ = MAX_TRAINING_SAMPLES;
    let _ = MAX_TXN_STAGING_KEYS;
    let _ = MAX_WRITE_SCOPE_KEYS;
    let _ = MIN_BASELINE_AGE_MS;
    let _ = OVERLAY_CAPACITY_DIVISOR;
    let _ = PAGE_MERGE_RATIO;
    let _ = PK_DELTA_ANCHOR_THRESHOLD;
    let _ = POOL_CAPACITY;
    let _ = PURGE_DELETE_BATCH;
    let _ = REGION_MERGE_MIN_DENSITY;
    let _ = ROWS_PER_PAGE;
    let _ = SATURATION_QUERIES;
    let _ = SCAN_PAGE;
    let _ = SHARD_FRAGMENTATION_THRESHOLD;
    let _ = SINGLE_SHARD_MAX_ESTIMATE;
    let _ = STABLE_ROW_ID_HOLE_WATERMARK;
    let _ = STUCK_SNAPSHOT_AGE_SECS;
    let _ = TOMBSTONE_REUSE_SCAN_BOUND;
    let _ = WAL_CHECKPOINT_THRESHOLD;
    let _ = ZONE_MAP_CHUNK_ROWS;
    let _ = ZONE_STALE_REBUILD_THRESHOLD;
}

#[test]
fn test_layer_a_defaults_match_storage_config() {
    let cfg = linkrs_config::StorageConfig::default();
    let cases: &[(&str, ThresholdValue)] = &[
        (
            "ResourceConfig.max_memory_bytes",
            ThresholdValue::U64(cfg.max_memory_bytes),
        ),
        (
            "ResourceConfig.index_memory_bytes",
            ThresholdValue::U64(cfg.index_memory_bytes),
        ),
        (
            "ResourceConfig.memory_soft_ratio",
            ThresholdValue::F64(cfg.memory_soft_ratio),
        ),
        (
            "ResourceConfig.memory_hard_ratio",
            ThresholdValue::F64(cfg.memory_hard_ratio),
        ),
        (
            "ResourceConfig.index_pool_capacity_bytes",
            ThresholdValue::U64(cfg.index_pool_capacity_bytes),
        ),
        (
            "ResourceConfig.index_eviction_high_ratio",
            ThresholdValue::F64(cfg.index_eviction_high_ratio),
        ),
        (
            "ResourceConfig.index_eviction_low_ratio",
            ThresholdValue::F64(cfg.index_eviction_low_ratio),
        ),
        (
            "PropertyGraphConfig.cache_memory",
            ThresholdValue::Usize(cfg.cache_memory_bytes as usize),
        ),
        (
            "ResourceConfig.cache_ttl",
            ThresholdValue::U64(cfg.cache_ttl_secs),
        ),
        (
            "ResourceConfig.cache_tti",
            ThresholdValue::U64(cfg.cache_tti_secs),
        ),
        (
            "ResourceConfig.dirty_flush_operations",
            ThresholdValue::U64(cfg.dirty_flush_operations),
        ),
        (
            "ResourceConfig.dirty_flush_bytes",
            ThresholdValue::U64(cfg.dirty_flush_bytes),
        ),
        (
            "PersistenceConfig.auto_checkpoint_interval",
            ThresholdValue::U64(cfg.checkpoint_interval_secs),
        ),
        (
            "FreezeConfig.delta_edge_threshold",
            ThresholdValue::U64(cfg.freeze_delta_edge_threshold),
        ),
        (
            "FreezeConfig.delta_memory_threshold_bytes",
            ThresholdValue::U64(cfg.freeze_delta_memory_threshold_bytes),
        ),
        (
            "FreezeConfig.deletion_threshold",
            ThresholdValue::F64(cfg.freeze_deletion_threshold),
        ),
        (
            "AutoCompactConfig.min_holes",
            ThresholdValue::U64(u64::from(cfg.auto_compact_min_holes)),
        ),
        (
            "AutoCompactConfig.min_hole_ratio",
            ThresholdValue::F32(cfg.auto_compact_min_hole_ratio as f32),
        ),
        (
            "AutoCompactConfig.min_interval_secs",
            ThresholdValue::U64(cfg.auto_compact_min_interval_secs),
        ),
        (
            "EdgeTableConfig.max_mutable_csr_bytes",
            ThresholdValue::Usize(cfg.edge_max_mutable_csr_bytes as usize),
        ),
        (
            "ResourceConfig.max_active_snapshots",
            ThresholdValue::Usize(cfg.max_active_snapshots),
        ),
        (
            "ResourceConfig.max_snapshot_age",
            ThresholdValue::U64(cfg.max_snapshot_age_secs),
        ),
        (
            "ResourceConfig.max_tombstones",
            ThresholdValue::Usize(cfg.max_tombstones),
        ),
        (
            "ResourceConfig.max_tombstone_bytes",
            ThresholdValue::U64(cfg.max_tombstone_bytes),
        ),
        (
            "ResourceConfig.index_gc_batch",
            ThresholdValue::Usize(cfg.index_gc_batch),
        ),
        (
            "ResourceConfig.operation_timeout",
            ThresholdValue::U64(cfg.operation_timeout_secs),
        ),
        (
            "EncodingThresholds.string_min_rows",
            ThresholdValue::Usize(cfg.string_min_rows),
        ),
        (
            "EncodingThresholds.avg_length_threshold",
            ThresholdValue::Usize(cfg.avg_length_threshold),
        ),
        (
            "EncodingThresholds.cardinality_ratio_threshold",
            ThresholdValue::F64(cfg.cardinality_ratio_threshold),
        ),
        (
            "EncodingThresholds.fsst_rebuild_threshold",
            ThresholdValue::F64(cfg.fsst_rebuild_threshold),
        ),
    ];
    for (id, config_value) in cases {
        let entry = entry(id);
        assert!(
            entry.layers.config_file,
            "{id} is checked against StorageConfig but does not declare the config_file surface",
        );
        assert_eq!(
            entry.documented_default, *config_value,
            "documented default for {id} drifted from StorageConfig",
        );
    }
}

#[test]
fn test_anchor_threshold_scales_with_live_rows() {
    assert_eq!(
        IdManager::anchor_threshold_for_live(0),
        PK_DELTA_ANCHOR_THRESHOLD,
    );
    assert_eq!(IdManager::anchor_threshold_for_live(32_768), 8_192);
    assert_eq!(IdManager::anchor_threshold_for_live(1 << 20), 262_144);
}
