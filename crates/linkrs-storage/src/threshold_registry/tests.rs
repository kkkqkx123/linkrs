//! Registry gate tests: defaults, bench coverage and shape.
//!
//! Large-file tests live here so the registry root stays a thin surface.

use super::bench_ref::find_bench_file;
use super::entries;
use super::model::ThresholdValue;
use crate::cache::buffer_pool::MAX_POOL_SHARDS;
use crate::compression::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};
use crate::cursor::ScanOptions;
use crate::edge::csr_shared::{
    DEFAULT_VERTEX_CAPACITY, SEGMENT_SHIFT, SEGMENT_SIZE, VERTEX_GROWTH_FACTOR,
};
use crate::edge::csr_with_properties::FSST_MAX_SYMBOLS;
use crate::edge::edge_table::compaction::MAX_VERTEX_RECLAIM_PER_PASS;
use crate::edge::edge_table::core::index::INDEX_SEGMENT_CUTOVER;
use crate::edge::edge_table::iterator::DEFAULT_ADJACENCY_BATCH;
use crate::edge::edge_table::wal::MAX_ENTRY_LEN;
use crate::edge::fragmentation_stats::GROUP_FRAGMENTATION_THRESHOLD;
use crate::edge::mutable_csr::core::DEFAULT_EDGE_CAPACITY;
use crate::edge::mutable_csr::live_set::LIVE_SET_WIDTH_BOUND;
use crate::edge::mutable_csr::overflow::OVERFLOW_REPACK_CHUNKS_PER_VERTEX;
use crate::edge::mutable_csr::row::{
    HIGH_DEGREE_CHUNK_EDGES, HIGH_DEGREE_LIVE_THRESHOLD, HIGH_DEGREE_SPARE_DIVISOR,
    MIN_ROW_CAPACITY, OVERFLOW_CHUNK_MAX, OVERFLOW_CHUNK_MIN, PACKED_CSR_DENSITY,
};
use crate::edge::mutable_csr::write::TOMBSTONE_REUSE_SCAN_BOUND;
use crate::edge::node_group::address::{LEAF_HIGH_CSR_DENSITY, LEAF_REGION_ROWS};
use crate::edge::node_group::{
    DEFAULT_NODE_GROUP_BITS, GROUP_MERGE_MIN_DENSITY, REGION_MERGE_MIN_DENSITY, ROUTE_CACHE_SLOTS,
};
use crate::edge::property_schema::DEFAULT_VERSION_CHAIN_CAP;
use crate::edge::pure_csr::LIVE_SET_WIDTH_BOUND as PURE_CSR_LIVE_SET_WIDTH_BOUND;
use crate::encoding::constant::MAX_CONSTANT_OVERRIDES;
use crate::encoding::fsst::{
    DEFAULT_REBUILD_THRESHOLD as FSST_DEFAULT_REBUILD_THRESHOLD, MAX_NGRAMS_PER_STRING,
    MAX_SYMBOL_LEN, MAX_TRAINING_SAMPLES, MIN_SYMBOL_LEN, SYMBOL_TABLE_SIZE,
};
use crate::engine::graph_storage::context::conflict::{
    COMMITTED_WRITE_SET_CAPACITY, MAX_PUBLISHED_WRITE_SET_SIZE,
};
use crate::engine::graph_storage::context::txn_staging::MAX_TXN_STAGING_KEYS;
use crate::engine::graph_storage::index_manager::checkpoint::POOL_CAPACITY;
use crate::engine::graph_storage::reader::edge::SCAN_PAGE;
use crate::engine::graph_storage::ttl::PURGE_DELETE_BATCH;
use crate::engine::graph_storage::GraphStorage;
use crate::index::chunk::data::CHUNK_TARGET_SIZE;
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
    OVERLAY_RECODE_MEMORY_FLOOR, OVERLAY_RECODE_MEMORY_RATIO_DEN, OVERLAY_RECODE_MEMORY_RATIO_NUM,
};
use crate::vertex::column::column::{EVICTION_SEGMENT_BYTES, MAX_BACKGROUND_LOAD_CHUNKS};
use crate::vertex::column::fixed_width::{FIXED_STRING_INLINE_LIMIT, VECTOR_DENSE_FIXED_MAX_DIM};
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
    DEFAULT_SEGMENT_SLOTS_BITS, MAX_SHARDS, SINGLE_SHARD_MAX_ESTIMATE,
};
use crate::vertex::write_scope::MAX_WRITE_SCOPE_KEYS;

#[test]
fn test_documented_defaults_match_live_values() {
    for entry in super::entries::all_entries() {
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
    for entry in super::entries::all_entries() {
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
    assert_eq!(
        super::entries::all_entries().count(),
        super::entries::REGISTRY_LEN,
        "flattened entry count drifted from the domain slice total",
    );
    let mut ids: Vec<&str> = super::entries::all_entries()
        .map(|entry| entry.id)
        .collect();
    ids.sort_unstable();
    let count = ids.len();
    ids.dedup();
    assert_eq!(count, ids.len(), "threshold ids must be unique: {ids:?}");
    for entry in super::entries::all_entries() {
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
    let _ = CHUNK_TARGET_SIZE;
    let _ = CRITICAL_MARGIN;
    let _ = DEFAULT_ADJACENCY_BATCH;
    let _ = DEFAULT_ALP_EXCEPTION_THRESHOLD;
    let _ = ScanOptions::DEFAULT_BATCH_SIZE;
    let _ = DEFAULT_CHUNK_ROWS;
    let _ = DEFAULT_CRITICAL_PCT;
    let _ = DEFAULT_DICT_MAX_ENTRIES_PER_CHUNK;
    let _ = DEFAULT_EDGE_CAPACITY;
    let _ = FIXED_STRING_INLINE_LIMIT;
    let _ = FSST_MAX_SYMBOLS;
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
    let _ = HIGH_DEGREE_CHUNK_EDGES;
    let _ = HIGH_DEGREE_LIVE_THRESHOLD;
    let _ = HIGH_DEGREE_SPARE_DIVISOR;
    let _ = ID_STRIPE_COUNT;
    let _ = INDEX_SEGMENT_CUTOVER;
    let _ = MAX_BACKGROUND_LOAD_CHUNKS;
    let _ = MAX_BASELINE_AGE_MS;
    let _ = MAX_CAPACITY;
    let _ = MAX_ENTRY_LEN;
    let _ = MAX_NGRAMS_PER_STRING;
    let _ = MAX_PAGE_SIZE;
    let _ = MAX_PUBLISHED_WRITE_SET_SIZE;
    let _ = MAX_SYMBOL_LEN;
    let _ = MAX_CONSTANT_OVERRIDES;
    let _ = MAX_POOL_SHARDS;
    let _ = MAX_SHARDS;
    let _ = MAX_TRAINING_SAMPLES;
    let _ = MAX_TXN_STAGING_KEYS;
    let _ = MAX_WRITE_SCOPE_KEYS;
    let _ = MAX_VERTEX_RECLAIM_PER_PASS;
    let _ = MIN_BASELINE_AGE_MS;
    let _ = MIN_ROW_CAPACITY;
    let _ = MIN_SYMBOL_LEN;
    let _ = OVERLAY_CAPACITY_DIVISOR;
    let _ = OVERLAY_RECODE_MEMORY_FLOOR;
    let _ = OVERLAY_RECODE_MEMORY_RATIO_NUM;
    let _ = OVERLAY_RECODE_MEMORY_RATIO_DEN;
    let _ = OVERFLOW_CHUNK_MAX;
    let _ = OVERFLOW_CHUNK_MIN;
    let _ = OVERFLOW_REPACK_CHUNKS_PER_VERTEX;
    let _ = PACKED_CSR_DENSITY;
    let _ = PAGE_MERGE_RATIO;
    let _ = PK_DELTA_ANCHOR_THRESHOLD;
    let _ = POOL_CAPACITY;
    let _ = PURGE_DELETE_BATCH;
    let _ = PURE_CSR_LIVE_SET_WIDTH_BOUND;
    let _ = GraphStorage::RECOMMENDED_BATCH_CHUNK_EDGES;
    let _ = REGION_MERGE_MIN_DENSITY;
    let _ = ROUTE_CACHE_SLOTS;
    let _ = ROWS_PER_PAGE;
    let _ = SATURATION_QUERIES;
    let _ = SCAN_PAGE;
    let _ = SEGMENT_SHIFT;
    let _ = SEGMENT_SIZE;
    let _ = SHARD_FRAGMENTATION_THRESHOLD;
    let _ = SINGLE_SHARD_MAX_ESTIMATE;
    let _ = STABLE_ROW_ID_HOLE_WATERMARK;
    let _ = STUCK_SNAPSHOT_AGE_SECS;
    let _ = SYMBOL_TABLE_SIZE;
    let _ = TOMBSTONE_REUSE_SCAN_BOUND;
    let _ = WAL_CHECKPOINT_THRESHOLD;
    let _ = DEFAULT_VERTEX_CAPACITY;
    let _ = VECTOR_DENSE_FIXED_MAX_DIM;
    let _ = VERTEX_GROWTH_FACTOR;
    let _ = LEAF_HIGH_CSR_DENSITY;
    let _ = LEAF_REGION_ROWS;
    let _ = LIVE_SET_WIDTH_BOUND;
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
        let entry = super::entries::entry(id);
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

#[test]
fn test_registry_sources_exist() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in entries::all_entries() {
        if entry.source.starts_with("linkrs-") {
            continue;
        }
        assert!(
            root.join(entry.source).exists(),
            "threshold {} points at missing source file {}",
            entry.id,
            entry.source,
        );
    }
}
