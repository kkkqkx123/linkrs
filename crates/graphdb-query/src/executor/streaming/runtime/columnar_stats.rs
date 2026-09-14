//! Query-level columnar fast-path counters (observability).
//!
//! Shared via `Arc` with the chunks produced by source operators; every
//! `DataChunk::evaluate_expression` call records a hit (columnar batch path
//! succeeded) or a miss (fell back to per-row evaluation). The miss rate
//! exposes how much of the flat-column promise is actually kept.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Constants for typed-column analysis.
///
/// These are exported so that benchmark harnesses can emit machine-readable
/// status lines that mirror the selection logic.
pub const D1_EVAL_THRESHOLD: u64 = 1_000_000;
pub const D1_TYPED_RATE_THRESHOLD: f64 = 0.5;

/// Closed enumeration of opaque operator boundaries that materialize a
/// selection vector.  Each entry pairs a display name (used by callers via
/// `materialize_selection_by("Name")`) with a per-query counter slot.
///
/// Keeping this list static lets the hot path attribute a materialization
/// with a cheap index lookup instead of a `Mutex<HashMap>` write, while the
/// snapshot routine still emits the same `HashMap<&'static str, u64>` shape
/// that PROFILE tooling already consumes.
///
/// New callers of [`DataChunk::materialize_selection_by`] must append their
/// label here (the lookup falls back to the unattributed path for unknown
/// names, but adding the label keeps per-operator visibility).
pub const SELECTION_BOUNDARY_OPS: &[&str] = &[
    "Filter",
    "Dedup",
    "Assign",
    "Remove",
    "Unwind",
    "AppendVertices",
    "Sample",
    "Root",
    "Engine",
    "Sort",
    "WindowFunction",
    "Window",
    "TopN",
    "Distinct",
    "Aggregate",
    "GroupBy",
    "Materialize",
    "DataCollect",
    "RollUpApply",
    "Sink",
    "CopyFrom",
    "CopyTo",
    "Exchange",
    "CrossSemiJoin",
    "NestedLoopJoin",
    "HashJoin",
    "MergeJoin",
    "Apply",
    "CorrelatedApply",
    "ShuffleJoin",
    "Set",
    "VectorSearch",
    "WcoBuild",
    "RecursiveFragment",
    "Fulltext",
    "Subgraph",
    "Gather",
    "Subquery",
];

/// Number of distinct opaque-boundary operator labels.
const N_BOUNDARY_OPS: usize = SELECTION_BOUNDARY_OPS.len();

/// Index of an opaque-boundary label inside [`SELECTION_BOUNDARY_OPS`], or
/// `None` for ad-hoc strings not part of the closed enumeration.
fn boundary_op_index(op: &'static str) -> Option<usize> {
    SELECTION_BOUNDARY_OPS.iter().position(|n| *n == op)
}

/// Query-level columnar fast-path counters (observability).
///
/// Shared via `Arc` with the chunks produced by source operators; every
/// `DataChunk::evaluate_expression` call records a hit (columnar batch path
/// succeeded) or a miss (fell back to per-row evaluation). The miss rate
/// exposes how much of the flat-column promise is actually kept.
///
/// `columnar_typed_hits` additionally counts evaluations served by the typed
/// batch fast path (raw `Vec<i64>`/`Vec<f64>`/`Vec<i32>` buffers).
/// Selection-vector counters record how often chunks are handed downstream
/// with a selection attached vs. materialized at a boundary.
#[derive(Debug)]
pub struct ColumnarStats {
    pub columnar_hits: AtomicU64,
    pub columnar_misses: AtomicU64,
    /// Evaluations served by the typed batch fast path.
    pub columnar_typed_hits: AtomicU64,
    /// Chunks handed downstream with a selection vector attached.
    pub selection_attached: AtomicU64,
    /// Chunks materialized at a selection boundary.
    pub selection_materialized: AtomicU64,
    /// evaluations served by the selection-aware visible-row fast path
    /// (selection vector consumed without materializing).
    pub selection_pushed: AtomicU64,
    /// per-operator materialization counters, indexed by the position of
    /// the label inside [`SELECTION_BOUNDARY_OPS`].  Lock-free on the hot
    /// path; only the snapshot iterates the array (no per-call Mutex).
    selection_materialized_by_op: [AtomicU64; N_BOUNDARY_OPS],
    /// Chunks produced by a source via the storage column-block path (A1).
    pub column_block_hits: AtomicU64,
    /// Logical rows produced by expanding `DataChunk::multiplicity` at opaque
    /// boundaries (`normalize_for_opaque`). Compared against stored rows it
    /// yields the expanded/stored ratio for PROFILE observability.
    pub multiplicity_expanded: AtomicU64,
    /// Rows written to spill runs (all operators, incl. terminal collector).
    pub spill_rows: AtomicU64,
    /// Bytes written to spill runs (on-disk file sizes).
    pub spill_bytes: AtomicU64,
    /// Number of spill runs finalized.
    pub spill_runs: AtomicU64,
    /// Chunks emitted through the batch-slice outlet path.
    pub batch_outlets: AtomicU64,
    /// Expressions served by the typed columnar fast path on the compiled
    /// (project/filter) hot path — i.e. the compiled-over-rows path was
    /// bypassed because the chunk carried a usable typed layout. Distinct
    /// from `columnar_typed_hits`, which counts the scalar chunk evaluator.
    pub columnar_b_path_hits: AtomicU64,
    /// Typed layouts built by a producer that were dropped before any
    /// downstream consumer read them (`take_rows_for_reuse`, join build
    /// consumption). A high count means typed columns were built in vain.
    pub columnar_wasted_builds: AtomicU64,
}

impl Default for ColumnarStats {
    fn default() -> Self {
        Self {
            columnar_hits: AtomicU64::new(0),
            columnar_misses: AtomicU64::new(0),
            columnar_typed_hits: AtomicU64::new(0),
            selection_attached: AtomicU64::new(0),
            selection_materialized: AtomicU64::new(0),
            selection_pushed: AtomicU64::new(0),
            selection_materialized_by_op: [const { AtomicU64::new(0) }; N_BOUNDARY_OPS],
            column_block_hits: AtomicU64::new(0),
            multiplicity_expanded: AtomicU64::new(0),
            spill_rows: AtomicU64::new(0),
            spill_bytes: AtomicU64::new(0),
            spill_runs: AtomicU64::new(0),
            batch_outlets: AtomicU64::new(0),
            columnar_b_path_hits: AtomicU64::new(0),
            columnar_wasted_builds: AtomicU64::new(0),
        }
    }
}

impl ColumnarStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_hit(&self) {
        self.columnar_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.columnar_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an evaluation served by the typed batch fast path.
    pub fn record_typed_hit(&self) {
        self.columnar_typed_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an expression served by the typed columnar fast path on the
    /// compiled (project/filter) hot path.
    pub fn record_b_path_hit(&self) {
        self.columnar_b_path_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a typed layout dropped before any downstream consumer read it.
    pub fn record_wasted_build(&self) {
        self.columnar_wasted_builds.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a chunk handed downstream with a selection vector.
    pub fn record_selection_attached(&self) {
        self.selection_attached.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a chunk materialized at a selection boundary.
    pub fn record_selection_materialized(&self) {
        self.selection_materialized.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a chunk materialized at a named operator boundary.
    ///
    /// The hot path is lock-free: it bumps the global counter and, if `op`
    /// belongs to the closed [`SELECTION_BOUNDARY_OPS`] enumeration, the
    /// matching per-operator atomic slot.  Unknown labels still count toward
    /// the global materialization total but are not attributed per operator.
    pub fn record_selection_materialized_by(&self, op: &'static str) {
        self.selection_materialized.fetch_add(1, Ordering::Relaxed);
        if let Some(idx) = boundary_op_index(op) {
            self.selection_materialized_by_op[idx].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// record an evaluation served by the selection-aware visible-row
    /// fast path (selection consumed in place, upstream chunk untouched).
    pub fn record_selection_pushed(&self) {
        self.selection_pushed.fetch_add(1, Ordering::Relaxed);
    }

    /// materialization sites, attributed per operator.
    pub fn materialized_by_operator(&self) -> HashMap<&'static str, u64> {
        SELECTION_BOUNDARY_OPS
            .iter()
            .zip(self.selection_materialized_by_op.iter())
            .filter(|(_, c)| c.load(Ordering::Relaxed) > 0)
            .map(|(name, c)| (*name, c.load(Ordering::Relaxed)))
            .collect()
    }

    /// Record a chunk produced via the storage column-block path (A1).
    pub fn record_column_block_hit(&self) {
        self.column_block_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record `n` logical rows produced by multiplicity expansion.
    pub fn record_multiplicity_expanded(&self, n: u64) {
        if n > 0 {
            self.multiplicity_expanded.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// Record one finalized spill run.
    pub fn record_spill(&self, rows: u64, bytes: u64) {
        self.record_spill_with_runs(rows, bytes, 1);
    }

    /// Record one chunk emitted through the batch-slice outlet path.
    pub fn record_batch_outlet(&self) {
        self.batch_outlets.fetch_add(1, Ordering::Relaxed);
    }

    /// Record aggregated spill output covering `runs` finalized runs.
    ///
    /// Terminal collector paths finalize N runs but report once; passing the
    /// run count keeps `spill_runs` exact instead of undercounting to 1.
    pub fn record_spill_with_runs(&self, rows: u64, bytes: u64, runs: u64) {
        self.spill_rows.fetch_add(rows, Ordering::Relaxed);
        self.spill_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.spill_runs.fetch_add(runs, Ordering::Relaxed);
    }

    /// Fraction of evaluation calls that hit the columnar fast path.
    /// Returns 1.0 when nothing was evaluated (vacuous).
    pub fn hit_rate(&self) -> f64 {
        let hits = self.columnar_hits.load(Ordering::Relaxed);
        let misses = self.columnar_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            1.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Fraction of evaluations served by the typed batch fast path.
    pub fn typed_hit_rate(&self) -> f64 {
        let hits = self.columnar_hits.load(Ordering::Relaxed);
        let typed = self.columnar_typed_hits.load(Ordering::Relaxed);
        if hits == 0 {
            0.0
        } else {
            typed as f64 / hits as f64
        }
    }

    /// Selection passthrough rate: attached / (attached + materialized).
    /// 1.0 means every selected chunk reached its consumer without materializing.
    pub fn selection_passthrough_rate(&self) -> f64 {
        let attached = self.selection_attached.load(Ordering::Relaxed);
        let materialized = self.selection_materialized.load(Ordering::Relaxed);
        let total = attached + materialized;
        if total == 0 {
            0.0
        } else {
            attached as f64 / total as f64
        }
    }
}

/// Point-in-time snapshot of the columnar fast-path counters for PROFILE.
///
/// Surfaces how much of the evaluation work ran through the columnar batch
/// fast path vs. row-wise fallback, and how often storage column blocks /
/// selection vectors were used.  When `hit_rate` is high for typical
/// workloads, a typed column representation becomes unnecessary; a low
/// `typed_hit_rate` with sustained `columnar_misses` motivates revisiting it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ColumnarStatsSnapshot {
    pub columnar_hits: u64,
    pub columnar_misses: u64,
    pub columnar_typed_hits: u64,
    pub selection_attached: u64,
    pub selection_materialized: u64,
    pub selection_pushed: u64,
    pub column_block_hits: u64,
    pub multiplicity_expanded: u64,
    pub spill_rows: u64,
    pub spill_bytes: u64,
    pub spill_runs: u64,
    pub batch_outlets: u64,
    pub columnar_b_path_hits: u64,
    pub columnar_wasted_builds: u64,
}

impl ColumnarStatsSnapshot {
    pub fn from_stats(stats: &ColumnarStats) -> Self {
        Self {
            columnar_hits: stats.columnar_hits.load(Ordering::Relaxed),
            columnar_misses: stats.columnar_misses.load(Ordering::Relaxed),
            columnar_typed_hits: stats.columnar_typed_hits.load(Ordering::Relaxed),
            selection_attached: stats.selection_attached.load(Ordering::Relaxed),
            selection_materialized: stats.selection_materialized.load(Ordering::Relaxed),
            selection_pushed: stats.selection_pushed.load(Ordering::Relaxed),
            column_block_hits: stats.column_block_hits.load(Ordering::Relaxed),
            multiplicity_expanded: stats.multiplicity_expanded.load(Ordering::Relaxed),
            spill_rows: stats.spill_rows.load(Ordering::Relaxed),
            spill_bytes: stats.spill_bytes.load(Ordering::Relaxed),
            spill_runs: stats.spill_runs.load(Ordering::Relaxed),
            batch_outlets: stats.batch_outlets.load(Ordering::Relaxed),
            columnar_b_path_hits: stats.columnar_b_path_hits.load(Ordering::Relaxed),
            columnar_wasted_builds: stats.columnar_wasted_builds.load(Ordering::Relaxed),
        }
    }

    /// Fraction of evaluations served by the columnar batch fast path.
    pub fn hit_rate(&self) -> f64 {
        let total = self.columnar_hits + self.columnar_misses;
        if total == 0 {
            1.0
        } else {
            self.columnar_hits as f64 / total as f64
        }
    }

    /// Fraction of evaluations served by the typed batch fast path.
    pub fn typed_hit_rate(&self) -> f64 {
        if self.columnar_hits == 0 {
            0.0
        } else {
            self.columnar_typed_hits as f64 / self.columnar_hits as f64
        }
    }

    /// Spilled rows per stored row for a caller-supplied stored-row count.
    ///
    /// Placed next to the expanded/stored ratio in PROFILE output so a large
    /// query shows spilled/stored at a glance. Returns 0.0 when there are no
    /// stored rows rather than dividing by zero.
    pub fn spilled_stored_ratio(&self, stored_rows: u64) -> f64 {
        if stored_rows == 0 {
            0.0
        } else {
            self.spill_rows as f64 / stored_rows as f64
        }
    }

    /// Human-readable one-line summary for PROFILE output.
    pub fn summary(&self) -> String {
        format!(
            "columnar_hits={}, misses={}, hit_rate={:.3}, typed_hit_rate={:.3}, b_path_hits={}, wasted_builds={}, selection_attached={}, selection_materialized={}, selection_pushed={}, column_block_hits={}, multiplicity_expanded={}, spill_rows={}, spill_bytes={}, spill_runs={}, batch_outlets={}",
            self.columnar_hits,
            self.columnar_misses,
            self.hit_rate(),
            self.typed_hit_rate(),
            self.columnar_b_path_hits,
            self.columnar_wasted_builds,
            self.selection_attached,
            self.selection_materialized,
            self.selection_pushed,
            self.column_block_hits,
            self.multiplicity_expanded,
            self.spill_rows,
            self.spill_bytes,
            self.spill_runs,
            self.batch_outlets,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_spilled_stored_ratio() {
        let stats = ColumnarStats::new();
        stats.record_spill_with_runs(200, 4096, 2);
        let snapshot = ColumnarStatsSnapshot::from_stats(&stats);
        assert_eq!(snapshot.spill_rows, 200);
        assert_eq!(snapshot.spill_bytes, 4096);
        assert_eq!(snapshot.spill_runs, 2);
        assert!((snapshot.spilled_stored_ratio(100) - 2.0).abs() < f64::EPSILON);
        assert_eq!(snapshot.spilled_stored_ratio(0), 0.0);
        assert!(snapshot.summary().contains("spill_rows=200"));
    }

    #[test]
    fn test_columnar_policy_flush_merges_into_shared_policy() {
        use crate::executor::streaming::chunk::ColumnarPolicy;
        use crate::executor::streaming::runtime::ExecutionRuntime;

        let policy = Arc::new(ColumnarPolicy::new(0.8, 100));
        let mut rt = ExecutionRuntime::default_budget();
        rt.set_columnar_policy(Some(policy.clone()));

        // Simulate a query: 10 columnar hits, 5 misses recorded on the
        // per-query counters, plus 4 compiled-hot-path (B path) hits.
        rt.columnar_stats().record_hit();
        for _ in 0..9 {
            rt.columnar_stats().record_hit();
        }
        for _ in 0..5 {
            rt.columnar_stats().record_miss();
        }
        for _ in 0..4 {
            rt.columnar_stats().record_b_path_hit();
        }
        assert_eq!(rt.columnar_stats().hit_rate(), 10.0 / 15.0);

        // Flushing merges the deltas into the shared policy, with B path
        // hits counted as effective hits.
        rt.flush_columnar_stats_to_policy();
        assert_eq!(policy.snapshot(), (14, 5));

        // Without an injected policy the flush is a no-op.
        let rt2 = ExecutionRuntime::default_budget();
        rt2.flush_columnar_stats_to_policy();
    }

    #[test]
    fn test_columnar_policy_gates_typed_columns() {
        use crate::executor::streaming::chunk::ColumnarPolicy;
        use crate::executor::streaming::runtime::ExecutionRuntime;

        let policy = Arc::new(ColumnarPolicy::new(0.8, 100));
        let mut rt = ExecutionRuntime::default_budget();
        rt.set_columnar_policy(Some(policy.clone()));

        // Below the sample floor the columnar path is chosen.
        let decide = || rt.columnar_policy().is_none_or(|p| p.should_use_columnar());
        assert!(decide());

        // Simulate a fallback-heavy workload: merge 90% misses across
        // queries; once the sample floor is crossed the decision flips.
        for _ in 0..4 {
            rt.flush_columnar_stats_to_policy();
            policy.merge(3, 30);
        }
        assert!(!decide());

        // Recovery: sustained hits flip the decision back.
        for _ in 0..10 {
            policy.merge(100, 0);
        }
        assert!(decide());
    }
}
