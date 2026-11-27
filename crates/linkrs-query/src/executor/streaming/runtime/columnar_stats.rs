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
    "CrossJoin",
    "SemiJoin",
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
    /// Chunks accumulated by the typed group-count state.
    pub typed_group_hits: AtomicU64,
    /// Chunks that matched the aggregate shape but could not use typed state.
    pub typed_group_fallbacks: AtomicU64,
    /// Wall-clock microseconds spent in storage scan batch reads.
    pub scan_read_us: AtomicU64,
    /// Candidate enumeration inside a storage scan batch read.
    pub scan_enumerate_us: AtomicU64,
    /// Table loading component of candidate enumeration.
    pub scan_table_load_us: AtomicU64,
    /// Window filtering component of candidate enumeration.
    pub scan_window_filter_us: AtomicU64,
    /// Shard-run merge component of candidate enumeration.
    pub scan_merge_us: AtomicU64,
    /// Repeated row-liveness checks on enumerated candidates.
    pub scan_visibility_us: AtomicU64,
    /// Column value decode inside a storage scan batch read.
    pub scan_decode_us: AtomicU64,
    /// Shard grouping and batch assembly inside a storage scan batch read.
    pub scan_assemble_us: AtomicU64,
    /// Zone pruning and residual predicate filtering inside a scan batch read.
    pub scan_predicate_us: AtomicU64,
    /// Query-side assembly of a storage column batch into a data chunk.
    pub scan_chunk_build_us: AtomicU64,
    /// Wall-clock microseconds spent in columnar adjacency and destination reads.
    pub expand_read_us: AtomicU64,
    /// Packed adjacency read inside a columnar expand batch.
    pub expand_topology_read_us: AtomicU64,
    /// Destination batch read inside a columnar expand batch.
    pub expand_destination_read_us: AtomicU64,
    /// Neighbor tag resolution before the destination read.
    pub expand_tag_resolve_us: AtomicU64,
    /// Snapshot and edge-table setup inside a handle-count storage read.
    pub expand_handle_setup_us: AtomicU64,
    /// Visible CSR walk inside a handle-count storage read.
    pub expand_handle_visit_us: AtomicU64,
    /// Endpoint ordering and aggregation inside a handle-count storage read.
    pub expand_handle_aggregate_us: AtomicU64,
    /// Query-side handle deduplication inside a count-only expand batch.
    pub expand_handle_traverse_us: AtomicU64,
    /// External key resolution for unique destination handles.
    pub expand_key_resolve_us: AtomicU64,
    /// Weight assembly after handle deduplication.
    pub expand_weight_us: AtomicU64,
    /// Seed extraction and handle collection before an expand storage read.
    pub expand_seed_us: AtomicU64,
    /// Cloning the incoming typed columns at expand entry.
    pub expand_typed_clone_us: AtomicU64,
    /// Schema, TTL and dangling-proof checks around an expand storage read.
    pub expand_guard_us: AtomicU64,
    /// Whole expand step dispatch, including work outside the columnar helper.
    pub expand_step_us: AtomicU64,
    /// Partition pipeline open time in parallel workers.
    pub partition_open_us: AtomicU64,
    /// Partition operator advance time in parallel workers.
    pub partition_advance_us: AtomicU64,
    /// Partition output queue send time in parallel workers.
    pub partition_send_us: AtomicU64,
    /// Partition pipeline close time in parallel workers.
    pub partition_close_us: AtomicU64,
    /// Destination deduplication, existence view and edge-order expansion.
    pub expand_view_us: AtomicU64,
    /// Typed or row output assembly after existence filtering.
    pub expand_output_us: AtomicU64,
    /// Number of batched columnar expand calls.
    pub expand_parent_batches: AtomicU64,
    /// Number of parent vertices processed by columnar expand.
    pub expand_parents: AtomicU64,
    /// Number of topology rows produced by columnar expand.
    pub expand_events: AtomicU64,
    /// Number of distinct destination ids read per batch.
    pub expand_unique_destinations: AtomicU64,
    /// Number of distinct destinations missing at the read timestamp.
    pub expand_missing_destinations: AtomicU64,
    /// Batches whose destination existence re-read was skipped under the
    /// dangling-free proof. The existence cost then drops to zero while
    /// the emitted rows stay identical.
    pub dangling_exist_skips: AtomicU64,
    pub frontier_first_hop_us: AtomicU64,
    pub frontier_sort_us: AtomicU64,
    pub frontier_resolve_us: AtomicU64,
    pub frontier_tail_us: AtomicU64,
    /// Number of rows emitted by columnar expand.
    pub expand_output_rows: AtomicU64,
    /// Candidate primary keys checked by an ordered existence scan.
    pub ordered_existence_candidates: AtomicU64,
    /// Candidates accepted by the reverse-edge existence gate.
    pub ordered_existence_matches: AtomicU64,
    /// Keys emitted by an ordered existence scan before its early stop.
    pub ordered_existence_output_rows: AtomicU64,
    /// Snapshot or generation drift observed by an ordered existence scan.
    pub ordered_existence_drift: AtomicU64,
    /// Wall-clock microseconds inside the ordered index cursor.
    pub ordered_existence_cursor_us: AtomicU64,
    /// Wall-clock microseconds inside the reverse-edge existence gate.
    pub ordered_existence_gate_us: AtomicU64,
    /// Wall-clock microseconds opening bounded index cursors.
    pub ordered_existence_open_us: AtomicU64,
    /// Number of bounded index cursor opens.
    pub ordered_existence_opens: AtomicU64,
    /// Rows rejected by the ordered existence contract (duplicate keys).
    pub ordered_existence_contract_violations: AtomicU64,
    /// Rows beyond the proven limit discarded by the contract boundary.
    pub ordered_existence_contract_discarded: AtomicU64,
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
            typed_group_hits: AtomicU64::new(0),
            typed_group_fallbacks: AtomicU64::new(0),
            scan_read_us: AtomicU64::new(0),
            scan_enumerate_us: AtomicU64::new(0),
            scan_table_load_us: AtomicU64::new(0),
            scan_window_filter_us: AtomicU64::new(0),
            scan_merge_us: AtomicU64::new(0),
            scan_visibility_us: AtomicU64::new(0),
            scan_decode_us: AtomicU64::new(0),
            scan_assemble_us: AtomicU64::new(0),
            scan_predicate_us: AtomicU64::new(0),
            scan_chunk_build_us: AtomicU64::new(0),
            expand_read_us: AtomicU64::new(0),
            expand_topology_read_us: AtomicU64::new(0),
            expand_destination_read_us: AtomicU64::new(0),
            expand_tag_resolve_us: AtomicU64::new(0),
            expand_handle_setup_us: AtomicU64::new(0),
            expand_handle_visit_us: AtomicU64::new(0),
            expand_handle_aggregate_us: AtomicU64::new(0),
            expand_handle_traverse_us: AtomicU64::new(0),
            expand_key_resolve_us: AtomicU64::new(0),
            expand_weight_us: AtomicU64::new(0),
            expand_seed_us: AtomicU64::new(0),
            expand_typed_clone_us: AtomicU64::new(0),
            expand_guard_us: AtomicU64::new(0),
            expand_step_us: AtomicU64::new(0),
            partition_open_us: AtomicU64::new(0),
            partition_advance_us: AtomicU64::new(0),
            partition_send_us: AtomicU64::new(0),
            partition_close_us: AtomicU64::new(0),
            expand_view_us: AtomicU64::new(0),
            expand_output_us: AtomicU64::new(0),
            expand_parent_batches: AtomicU64::new(0),
            expand_parents: AtomicU64::new(0),
            expand_events: AtomicU64::new(0),
            expand_unique_destinations: AtomicU64::new(0),
            expand_missing_destinations: AtomicU64::new(0),
            dangling_exist_skips: AtomicU64::new(0),
            frontier_first_hop_us: AtomicU64::new(0),
            frontier_sort_us: AtomicU64::new(0),
            frontier_resolve_us: AtomicU64::new(0),
            frontier_tail_us: AtomicU64::new(0),
            expand_output_rows: AtomicU64::new(0),
            ordered_existence_candidates: AtomicU64::new(0),
            ordered_existence_matches: AtomicU64::new(0),
            ordered_existence_output_rows: AtomicU64::new(0),
            ordered_existence_drift: AtomicU64::new(0),
            ordered_existence_cursor_us: AtomicU64::new(0),
            ordered_existence_gate_us: AtomicU64::new(0),
            ordered_existence_open_us: AtomicU64::new(0),
            ordered_existence_opens: AtomicU64::new(0),
            ordered_existence_contract_violations: AtomicU64::new(0),
            ordered_existence_contract_discarded: AtomicU64::new(0),
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

    /// Record a chunk consumed by the typed group-count state.
    pub fn record_typed_group_hit(&self) {
        self.typed_group_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a chunk that fell back from typed group counting.
    pub fn record_typed_group_fallback(&self) {
        self.typed_group_fallbacks.fetch_add(1, Ordering::Relaxed);
    }

    /// Add storage scan read time for one batch.
    pub fn record_scan_read_us(&self, micros: u64) {
        self.scan_read_us.fetch_add(micros, Ordering::Relaxed);
    }

    /// Add the disjoint phases of one storage scan batch read.
    pub fn record_scan_phases(
        &self,
        enumerate_us: u64,
        table_load_us: u64,
        window_filter_us: u64,
        merge_us: u64,
        visibility_us: u64,
        decode_us: u64,
        assemble_us: u64,
        predicate_us: u64,
    ) {
        self.scan_enumerate_us
            .fetch_add(enumerate_us, Ordering::Relaxed);
        self.scan_table_load_us
            .fetch_add(table_load_us, Ordering::Relaxed);
        self.scan_window_filter_us
            .fetch_add(window_filter_us, Ordering::Relaxed);
        self.scan_merge_us.fetch_add(merge_us, Ordering::Relaxed);
        self.scan_visibility_us
            .fetch_add(visibility_us, Ordering::Relaxed);
        self.scan_decode_us.fetch_add(decode_us, Ordering::Relaxed);
        self.scan_assemble_us
            .fetch_add(assemble_us, Ordering::Relaxed);
        self.scan_predicate_us
            .fetch_add(predicate_us, Ordering::Relaxed);
    }

    /// Add query-side column batch assembly time for one scan chunk.
    pub fn record_scan_chunk_build_us(&self, micros: u64) {
        self.scan_chunk_build_us
            .fetch_add(micros, Ordering::Relaxed);
    }

    /// Add columnar expand read time for one batch.
    pub fn record_expand_read_us(&self, micros: u64) {
        self.expand_read_us.fetch_add(micros, Ordering::Relaxed);
    }

    /// Add the disjoint phases of one columnar expand batch.
    pub fn record_expand_phases(
        &self,
        topology_us: u64,
        destination_us: u64,
        tag_us: u64,
        view_us: u64,
        output_us: u64,
    ) {
        self.expand_topology_read_us
            .fetch_add(topology_us, Ordering::Relaxed);
        self.expand_destination_read_us
            .fetch_add(destination_us, Ordering::Relaxed);
        self.expand_tag_resolve_us
            .fetch_add(tag_us, Ordering::Relaxed);
        self.expand_view_us.fetch_add(view_us, Ordering::Relaxed);
        self.expand_output_us
            .fetch_add(output_us, Ordering::Relaxed);
    }

    /// Record the disjoint phases of one worker partition execution.
    pub fn record_partition_phases(
        &self,
        open_us: u64,
        advance_us: u64,
        send_us: u64,
        close_us: u64,
    ) {
        self.partition_open_us.fetch_add(open_us, Ordering::Relaxed);
        self.partition_advance_us
            .fetch_add(advance_us, Ordering::Relaxed);
        self.partition_send_us.fetch_add(send_us, Ordering::Relaxed);
        self.partition_close_us
            .fetch_add(close_us, Ordering::Relaxed);
    }

    /// Add seed extraction time for one columnar expand batch.
    pub fn record_expand_seed_us(&self, micros: u64) {
        self.expand_seed_us.fetch_add(micros, Ordering::Relaxed);
    }

    /// Add typed column clone time for one columnar expand batch.
    pub fn record_expand_typed_clone_us(&self, micros: u64) {
        self.expand_typed_clone_us
            .fetch_add(micros, Ordering::Relaxed);
    }

    /// Add catalog and dangling-proof guard time for one expand batch.
    pub fn record_expand_guard_us(&self, micros: u64) {
        self.expand_guard_us.fetch_add(micros, Ordering::Relaxed);
    }

    /// Add whole expand step dispatch time for one input chunk.
    pub fn record_expand_step_us(&self, micros: u64) {
        self.expand_step_us.fetch_add(micros, Ordering::Relaxed);
    }

    /// Record the disjoint storage-side phases of a handle-count read.
    pub fn record_handle_visit_phases(&self, setup_us: u64, visit_us: u64, aggregate_us: u64) {
        self.expand_handle_setup_us
            .fetch_add(setup_us, Ordering::Relaxed);
        self.expand_handle_visit_us
            .fetch_add(visit_us, Ordering::Relaxed);
        self.expand_handle_aggregate_us
            .fetch_add(aggregate_us, Ordering::Relaxed);
    }

    /// Record the disjoint query-side phases of a handle-count expand.
    pub fn record_handle_phases(&self, traverse_us: u64, key_resolve_us: u64, weight_us: u64) {
        self.expand_handle_traverse_us
            .fetch_add(traverse_us, Ordering::Relaxed);
        self.expand_key_resolve_us
            .fetch_add(key_resolve_us, Ordering::Relaxed);
        self.expand_weight_us
            .fetch_add(weight_us, Ordering::Relaxed);
    }

    /// Record the topology cardinality of one columnar expand batch.
    pub fn record_expand_topology_counts(&self, parents: u64, events: u64) {
        self.expand_parent_batches.fetch_add(1, Ordering::Relaxed);
        self.expand_parents.fetch_add(parents, Ordering::Relaxed);
        self.expand_events.fetch_add(events, Ordering::Relaxed);
    }

    /// Record the destination cardinality of one columnar expand batch.
    pub fn record_expand_destination_counts(&self, unique: u64, missing: u64) {
        self.expand_unique_destinations
            .fetch_add(unique, Ordering::Relaxed);
        self.expand_missing_destinations
            .fetch_add(missing, Ordering::Relaxed);
    }

    /// Record one batch whose destination existence re-read was skipped
    /// under the dangling-free proof.
    pub fn record_dangling_exist_skip(&self) {
        self.dangling_exist_skips.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one fused two-hop frontier's phase timings.
    pub fn record_frontier_phases(
        &self,
        first_hop_us: u64,
        sort_us: u64,
        resolve_us: u64,
        tail_us: u64,
    ) {
        self.frontier_first_hop_us
            .fetch_add(first_hop_us, Ordering::Relaxed);
        self.frontier_sort_us.fetch_add(sort_us, Ordering::Relaxed);
        self.frontier_resolve_us
            .fetch_add(resolve_us, Ordering::Relaxed);
        self.frontier_tail_us.fetch_add(tail_us, Ordering::Relaxed);
    }

    /// Record the number of rows emitted by one columnar expand batch.
    pub fn record_expand_output_rows(&self, rows: u64) {
        self.expand_output_rows.fetch_add(rows, Ordering::Relaxed);
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
    pub typed_group_hits: u64,
    pub typed_group_fallbacks: u64,
    pub scan_read_us: u64,
    pub scan_enumerate_us: u64,
    pub scan_table_load_us: u64,
    pub scan_window_filter_us: u64,
    pub scan_merge_us: u64,
    pub scan_visibility_us: u64,
    pub scan_decode_us: u64,
    pub scan_assemble_us: u64,
    pub scan_predicate_us: u64,
    pub scan_chunk_build_us: u64,
    pub expand_read_us: u64,
    pub expand_topology_read_us: u64,
    pub expand_destination_read_us: u64,
    pub expand_tag_resolve_us: u64,
    pub expand_handle_setup_us: u64,
    pub expand_handle_visit_us: u64,
    pub expand_handle_aggregate_us: u64,
    pub expand_handle_traverse_us: u64,
    pub expand_key_resolve_us: u64,
    pub expand_weight_us: u64,
    pub expand_seed_us: u64,
    pub expand_typed_clone_us: u64,
    pub expand_guard_us: u64,
    pub expand_step_us: u64,
    pub partition_open_us: u64,
    pub partition_advance_us: u64,
    pub partition_send_us: u64,
    pub partition_close_us: u64,
    pub expand_view_us: u64,
    pub expand_output_us: u64,
    pub expand_parent_batches: u64,
    pub expand_parents: u64,
    pub expand_events: u64,
    pub expand_unique_destinations: u64,
    pub expand_missing_destinations: u64,
    pub dangling_exist_skips: u64,
    pub frontier_first_hop_us: u64,
    pub frontier_sort_us: u64,
    pub frontier_resolve_us: u64,
    pub frontier_tail_us: u64,
    pub expand_output_rows: u64,
    pub ordered_existence_candidates: u64,
    pub ordered_existence_matches: u64,
    pub ordered_existence_output_rows: u64,
    pub ordered_existence_drift: u64,
    pub ordered_existence_cursor_us: u64,
    pub ordered_existence_gate_us: u64,
    pub ordered_existence_open_us: u64,
    pub ordered_existence_opens: u64,
    pub ordered_existence_contract_violations: u64,
    pub ordered_existence_contract_discarded: u64,
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
            typed_group_hits: stats.typed_group_hits.load(Ordering::Relaxed),
            typed_group_fallbacks: stats.typed_group_fallbacks.load(Ordering::Relaxed),
            scan_read_us: stats.scan_read_us.load(Ordering::Relaxed),
            scan_enumerate_us: stats.scan_enumerate_us.load(Ordering::Relaxed),
            scan_table_load_us: stats.scan_table_load_us.load(Ordering::Relaxed),
            scan_window_filter_us: stats.scan_window_filter_us.load(Ordering::Relaxed),
            scan_merge_us: stats.scan_merge_us.load(Ordering::Relaxed),
            scan_visibility_us: stats.scan_visibility_us.load(Ordering::Relaxed),
            scan_decode_us: stats.scan_decode_us.load(Ordering::Relaxed),
            scan_assemble_us: stats.scan_assemble_us.load(Ordering::Relaxed),
            scan_predicate_us: stats.scan_predicate_us.load(Ordering::Relaxed),
            scan_chunk_build_us: stats.scan_chunk_build_us.load(Ordering::Relaxed),
            expand_read_us: stats.expand_read_us.load(Ordering::Relaxed),
            expand_topology_read_us: stats.expand_topology_read_us.load(Ordering::Relaxed),
            expand_destination_read_us: stats.expand_destination_read_us.load(Ordering::Relaxed),
            expand_tag_resolve_us: stats.expand_tag_resolve_us.load(Ordering::Relaxed),
            expand_handle_setup_us: stats.expand_handle_setup_us.load(Ordering::Relaxed),
            expand_handle_visit_us: stats.expand_handle_visit_us.load(Ordering::Relaxed),
            expand_handle_aggregate_us: stats.expand_handle_aggregate_us.load(Ordering::Relaxed),
            expand_handle_traverse_us: stats.expand_handle_traverse_us.load(Ordering::Relaxed),
            expand_key_resolve_us: stats.expand_key_resolve_us.load(Ordering::Relaxed),
            expand_weight_us: stats.expand_weight_us.load(Ordering::Relaxed),
            expand_seed_us: stats.expand_seed_us.load(Ordering::Relaxed),
            expand_typed_clone_us: stats.expand_typed_clone_us.load(Ordering::Relaxed),
            expand_guard_us: stats.expand_guard_us.load(Ordering::Relaxed),
            expand_step_us: stats.expand_step_us.load(Ordering::Relaxed),
            partition_open_us: stats.partition_open_us.load(Ordering::Relaxed),
            partition_advance_us: stats.partition_advance_us.load(Ordering::Relaxed),
            partition_send_us: stats.partition_send_us.load(Ordering::Relaxed),
            partition_close_us: stats.partition_close_us.load(Ordering::Relaxed),
            expand_view_us: stats.expand_view_us.load(Ordering::Relaxed),
            expand_output_us: stats.expand_output_us.load(Ordering::Relaxed),
            expand_parent_batches: stats.expand_parent_batches.load(Ordering::Relaxed),
            expand_parents: stats.expand_parents.load(Ordering::Relaxed),
            expand_events: stats.expand_events.load(Ordering::Relaxed),
            expand_unique_destinations: stats.expand_unique_destinations.load(Ordering::Relaxed),
            expand_missing_destinations: stats.expand_missing_destinations.load(Ordering::Relaxed),
            dangling_exist_skips: stats.dangling_exist_skips.load(Ordering::Relaxed),
            frontier_first_hop_us: stats.frontier_first_hop_us.load(Ordering::Relaxed),
            frontier_sort_us: stats.frontier_sort_us.load(Ordering::Relaxed),
            frontier_resolve_us: stats.frontier_resolve_us.load(Ordering::Relaxed),
            frontier_tail_us: stats.frontier_tail_us.load(Ordering::Relaxed),
            expand_output_rows: stats.expand_output_rows.load(Ordering::Relaxed),
            ordered_existence_candidates: stats
                .ordered_existence_candidates
                .load(Ordering::Relaxed),
            ordered_existence_matches: stats.ordered_existence_matches.load(Ordering::Relaxed),
            ordered_existence_output_rows: stats
                .ordered_existence_output_rows
                .load(Ordering::Relaxed),
            ordered_existence_drift: stats.ordered_existence_drift.load(Ordering::Relaxed),
            ordered_existence_cursor_us: stats.ordered_existence_cursor_us.load(Ordering::Relaxed),
            ordered_existence_gate_us: stats.ordered_existence_gate_us.load(Ordering::Relaxed),
            ordered_existence_open_us: stats.ordered_existence_open_us.load(Ordering::Relaxed),
            ordered_existence_opens: stats.ordered_existence_opens.load(Ordering::Relaxed),
            ordered_existence_contract_violations: stats
                .ordered_existence_contract_violations
                .load(Ordering::Relaxed),
            ordered_existence_contract_discarded: stats
                .ordered_existence_contract_discarded
                .load(Ordering::Relaxed),
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
            "columnar_hits={}, misses={}, hit_rate={:.3}, typed_hit_rate={:.3}, b_path_hits={}, wasted_builds={}, typed_group_hits={}, typed_group_fallbacks={}, scan_read_us={}, scan_enumerate_us={}, scan_table_load_us={}, scan_window_filter_us={}, scan_merge_us={}, scan_visibility_us={}, scan_decode_us={}, scan_assemble_us={}, scan_predicate_us={}, scan_chunk_build_us={}, expand_read_us={}, expand_topology_read_us={}, expand_destination_read_us={}, expand_tag_resolve_us={}, expand_handle_setup_us={}, expand_handle_visit_us={}, expand_handle_aggregate_us={}, expand_handle_traverse_us={}, expand_key_resolve_us={}, expand_weight_us={}, expand_seed_us={}, expand_typed_clone_us={}, expand_guard_us={}, expand_step_us={}, partition_open_us={}, partition_advance_us={}, partition_send_us={}, partition_close_us={}, expand_view_us={}, expand_output_us={}, expand_parent_batches={}, expand_parents={}, expand_events={}, expand_unique_destinations={}, expand_missing_destinations={}, dangling_exist_skips={}, frontier_first_hop_us={}, frontier_sort_us={}, frontier_resolve_us={}, frontier_tail_us={}, expand_output_rows={}, ordered_existence_candidates={}, ordered_existence_matches={}, ordered_existence_output_rows={}, ordered_existence_drift={}, ordered_existence_cursor_us={}, ordered_existence_gate_us={}, ordered_existence_open_us={}, ordered_existence_opens={}, ordered_existence_contract_violations={}, ordered_existence_contract_discarded={}, selection_attached={}, selection_materialized={}, selection_pushed={}, column_block_hits={}, multiplicity_expanded={}, spill_rows={}, spill_bytes={}, spill_runs={}, batch_outlets={}",
            self.columnar_hits,
            self.columnar_misses,
            self.hit_rate(),
            self.typed_hit_rate(),
            self.columnar_b_path_hits,
            self.columnar_wasted_builds,
            self.typed_group_hits,
            self.typed_group_fallbacks,
            self.scan_read_us,
            self.scan_enumerate_us,
            self.scan_table_load_us,
            self.scan_window_filter_us,
            self.scan_merge_us,
            self.scan_visibility_us,
            self.scan_decode_us,
            self.scan_assemble_us,
            self.scan_predicate_us,
            self.scan_chunk_build_us,
            self.expand_read_us,
            self.expand_topology_read_us,
            self.expand_destination_read_us,
            self.expand_tag_resolve_us,
            self.expand_handle_setup_us,
            self.expand_handle_visit_us,
            self.expand_handle_aggregate_us,
            self.expand_handle_traverse_us,
            self.expand_key_resolve_us,
            self.expand_weight_us,
            self.expand_seed_us,
            self.expand_typed_clone_us,
            self.expand_guard_us,
            self.expand_step_us,
            self.partition_open_us,
            self.partition_advance_us,
            self.partition_send_us,
            self.partition_close_us,
            self.expand_view_us,
            self.expand_output_us,
            self.expand_parent_batches,
            self.expand_parents,
            self.expand_events,
            self.expand_unique_destinations,
            self.expand_missing_destinations,
            self.dangling_exist_skips,
            self.frontier_first_hop_us,
            self.frontier_sort_us,
            self.frontier_resolve_us,
            self.frontier_tail_us,
            self.expand_output_rows,
            self.ordered_existence_candidates,
            self.ordered_existence_matches,
            self.ordered_existence_output_rows,
            self.ordered_existence_drift,
            self.ordered_existence_cursor_us,
            self.ordered_existence_gate_us,
            self.ordered_existence_open_us,
            self.ordered_existence_opens,
            self.ordered_existence_contract_violations,
            self.ordered_existence_contract_discarded,
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
    fn typed_group_counters_snapshot_and_render() {
        let stats = ColumnarStats::new();
        stats.record_typed_group_hit();
        stats.record_typed_group_hit();
        stats.record_typed_group_fallback();
        let snapshot = ColumnarStatsSnapshot::from_stats(&stats);
        assert_eq!(snapshot.typed_group_hits, 2);
        assert_eq!(snapshot.typed_group_fallbacks, 1);
        assert!(snapshot.summary().contains("typed_group_hits=2"));
        assert!(snapshot.summary().contains("typed_group_fallbacks=1"));
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
