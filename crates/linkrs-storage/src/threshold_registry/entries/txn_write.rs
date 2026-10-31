//! Transaction and write-path thresholds.

use super::super::model::{ThresholdEntry, ThresholdValue, LAYER_C};
use crate::edge::mutable_csr::write::TOMBSTONE_REUSE_SCAN_BOUND;
use crate::engine::graph_storage::context::conflict::{
    COMMITTED_WRITE_SET_CAPACITY, MAX_PUBLISHED_WRITE_SET_SIZE,
};
use crate::engine::graph_storage::context::txn_staging::MAX_TXN_STAGING_KEYS;
use crate::engine::graph_storage::GraphStorage;
use crate::vertex::write_scope::MAX_WRITE_SCOPE_KEYS;

/// Domain entries.
pub(crate) const ENTRIES: &[ThresholdEntry] = &[
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
    ThresholdEntry {
        id: "RECOMMENDED_BATCH_CHUNK_EDGES",
        unit: "count",
        source: "engine/graph_storage/writer/batch.rs",
        layers: LAYER_C,
        live_default: || ThresholdValue::Usize(GraphStorage::RECOMMENDED_BATCH_CHUNK_EDGES),
        documented_default: ThresholdValue::Usize(10_000),
        benches: &["run_commit_scale"],
        affected_modules: &["engine/graph_storage"],
        note: "recommended caller-side chunk size for very large edge batches; commit_gate_bench covers commit scaling across batch sizes",
    },
];
