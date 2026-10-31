//! Flush, checkpoint and WAL thresholds.

use super::super::model::{ThresholdEntry, ThresholdValue, LAYER_AB, LAYER_B, LAYER_C};
use crate::edge::edge_table::wal::MAX_ENTRY_LEN;
use crate::engine::config::{FlushConfig, ResourceConfig};
use crate::engine::persistence_coordinator::config::PersistenceConfig;
use crate::index::shard_runtime::shard::WAL_CHECKPOINT_THRESHOLD;
use crate::vertex::id_indexer::PK_DELTA_ANCHOR_THRESHOLD;
use crate::vertex::vertex_table::flush_trigger::{
    MAX_BASELINE_AGE_MS, MIN_BASELINE_AGE_MS, PAGE_MERGE_RATIO,
};

/// Domain entries.
pub(crate) const ENTRIES: &[ThresholdEntry] = &[
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
];
