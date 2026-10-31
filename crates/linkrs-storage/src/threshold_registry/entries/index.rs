//! Index thresholds.

use super::super::model::{ThresholdEntry, ThresholdValue, LAYER_B, LAYER_C};
use crate::edge::edge_table::core::index::INDEX_SEGMENT_CUTOVER;
use crate::edge::node_group::ROUTE_CACHE_SLOTS;
use crate::edge::property_schema::DEFAULT_VERSION_CHAIN_CAP;
use crate::engine::graph_storage::index_manager::checkpoint::POOL_CAPACITY;
use crate::index::chunk::data::CHUNK_TARGET_SIZE;
use crate::index::shard_runtime::bloom::SATURATION_QUERIES;
use crate::index::shard_runtime::runtime::DEFAULT_POOL_CAPACITY;
use crate::vertex::id_indexer::config::{
    DEFAULT_GROWTH_FACTOR, DEFAULT_INITIAL_CAPACITY, MAX_CAPACITY,
};
use crate::vertex::id_indexer::manager::ID_STRIPE_COUNT;

/// Domain entries.
pub(crate) const ENTRIES: &[ThresholdEntry] = &[
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
    ThresholdEntry {
        id: "CHUNK_TARGET_SIZE",
        unit: "bytes",
        source: "index/chunk/data.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(CHUNK_TARGET_SIZE),
        documented_default: ThresholdValue::Usize(65_536),
        benches: &[],
        affected_modules: &["index"],
        note: "index chunk target size; no bench isolates chunk sizing",
    },
    ThresholdEntry {
        id: "ROUTE_CACHE_SLOTS",
        unit: "count",
        source: "edge/node_group.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(ROUTE_CACHE_SLOTS),
        documented_default: ThresholdValue::Usize(128),
        benches: &[],
        affected_modules: &["edge/node_group"],
        note: "hot-row routing cache slots; no bench isolates the group-address cache hit rate",
    },
];
