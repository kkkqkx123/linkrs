//! Cache thresholds.

use super::super::model::{ThresholdEntry, ThresholdValue, LAYER_AB, LAYER_B, LAYER_C};
use crate::cache::buffer_pool::MAX_POOL_SHARDS;
use crate::cache::config::RecordCacheConfig;
use crate::engine::config::{PropertyGraphConfig, ResourceConfig};

/// Domain entries.
pub(crate) const ENTRIES: &[ThresholdEntry] = &[
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
    ThresholdEntry {
        id: "MAX_POOL_SHARDS",
        unit: "count",
        source: "cache/buffer_pool.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(MAX_POOL_SHARDS),
        documented_default: ThresholdValue::Usize(16),
        benches: &[],
        affected_modules: &["cache"],
        note: "buffer pool shard-map ceiling; effective shards adapt to CPU parallelism within this bound",
    },
];
