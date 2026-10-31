//! Snapshot, GC and tombstone thresholds.

use super::super::model::{ThresholdEntry, ThresholdValue, LAYER_AB, LAYER_B, LAYER_C};
use crate::edge::edge_table::config::AutoMaintenanceConfig;
use crate::engine::config::{PropertyGraphConfig, ResourceConfig};
use crate::engine::graph_storage::ttl::PURGE_DELETE_BATCH;
use crate::vertex::gc_manager::STUCK_SNAPSHOT_AGE_SECS;

/// Domain entries.
pub(crate) const ENTRIES: &[ThresholdEntry] = &[
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
];
