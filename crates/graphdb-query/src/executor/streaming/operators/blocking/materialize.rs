use std::collections::HashSet;
use std::sync::Arc;

use crate::executor::streaming::slot::SlotLayout;
use crate::executor::streaming::spill::{HashPartitionSpiller, RunReader, SpilledRun};
use graphdb_core::columnar::{MaterializedBatch, RowKey};
use graphdb_core::Value;

#[derive(Debug)]
pub struct DistinctState {
    /// Dedup keys: the key-column projection of each unique row. Empty
    /// `key_cols` means the full row, preserving the exact `Value` Hash/Eq
    /// contract (Null, NaN, mixed kinds) of the former `HashSet<Vec<Value>>`.
    pub seen_rows: HashSet<RowKey>,
    /// Key column positions. Empty selects the full row. Reserved for future
    /// keyed distinct pushdown; no producer sets it yet.
    pub key_cols: Vec<usize>,
    /// Output buffer built once from drained keys at emission time, so the
    /// outlet serves batch slices instead of a drained row iterator.
    pub batch: MaterializedBatch,
    /// Output cursor into `batch`.
    pub emitted_offset: usize,
    pub col_names: Vec<String>,
    pub input_layout: Option<Arc<SlotLayout>>,
    pub partition_spiller: Option<HashPartitionSpiller>,
    pub spilled_runs: Vec<Option<SpilledRun>>,
    pub current_partition: usize,
    pub partition_seen: HashSet<RowKey>,
    pub has_spilled: bool,
}

/// Project the dedup key of a row.
///
/// Empty `key_cols` clones the full row (identity projection, the only mode
/// with a producer today). A future keyed distinct sets column positions to
/// dedup by key while emitting full rows.
pub(crate) fn project_key(row: &[Value], key_cols: &[usize]) -> RowKey {
    if key_cols.is_empty() {
        row.to_vec()
    } else {
        key_cols.iter().map(|&c| row[c].clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::value::NullType;

    fn sample_rows() -> Vec<Vec<Value>> {
        vec![
            vec![Value::BigInt(1), Value::string("k")],
            vec![Value::BigInt(1), Value::string("k")],
            vec![Value::BigInt(2), Value::string("k")],
            vec![Value::Null(NullType::Null), Value::string("k")],
            vec![Value::Null(NullType::Null), Value::string("k")],
            vec![Value::Double(f64::NAN), Value::Int(1)],
            vec![Value::Int(1), Value::BigInt(1)],
            vec![Value::Int(1), Value::BigInt(1)],
        ]
    }

    fn dedup_reference(rows: &[Vec<Value>]) -> Vec<Vec<Value>> {
        let mut seen: HashSet<Vec<Value>> = HashSet::new();
        let mut out = Vec::new();
        for row in rows {
            if seen.insert(row.clone()) {
                out.push(row.clone());
            }
        }
        out
    }

    fn dedup_keyed(rows: &[Vec<Value>], key_cols: &[usize]) -> Vec<Vec<Value>> {
        let mut seen: HashSet<RowKey> = HashSet::new();
        let mut batch = MaterializedBatch::new(0, 0);
        for row in rows {
            if seen.insert(project_key(row, key_cols)) {
                batch.append_row(row.clone());
            }
        }
        batch.to_rows()
    }

    #[test]
    fn identity_projection_matches_row_reference() {
        let rows = sample_rows();
        let mut keyed = dedup_keyed(&rows, &[]);
        let mut reference = dedup_reference(&rows);
        // Both hold the same multiset; HashSet iteration order is arbitrary.
        keyed.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        reference.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        // NaN never equals itself under `Value` equality, so both keep every
        // NaN row; compare lengths and non-NaN members instead of full eq.
        assert_eq!(keyed.len(), reference.len());
        for row in &reference {
            assert!(
                keyed.iter().any(|k| k == row),
                "reference row missing from keyed output: {row:?}"
            );
        }
    }

    #[test]
    fn key_subset_dedups_by_key_columns_only() {
        let rows = sample_rows();
        let keyed = dedup_keyed(&rows, &[1]);
        // Column 1 values: "k" x5, Int(1) x1, BigInt(1) x2 → 3 groups.
        assert_eq!(keyed.len(), 3);
        assert_eq!(keyed[0], rows[0]);
        // Full-row mode agrees with the row reference implementation.
        assert_eq!(dedup_keyed(&rows, &[]).len(), dedup_reference(&rows).len());
    }

    #[test]
    fn project_key_empty_cols_is_identity() {
        let row = vec![Value::BigInt(1), Value::Null(NullType::Null)];
        assert_eq!(project_key(&row, &[]), row);
        assert_eq!(project_key(&row, &[1]), vec![Value::Null(NullType::Null)]);
    }
}

#[derive(Debug)]
pub struct MaterializeState {
    /// Column-oriented materialized state (was `Vec<Vec<Value>>`).
    pub batch: MaterializedBatch,
    /// Output cursor into `batch`; replaces the drained row iterator so the
    /// batch stays intact for future spill replay.
    pub emitted_offset: usize,
    pub materialized: bool,
    pub input_layout: Option<Arc<SlotLayout>>,
    /// Ordered spill runs (creation order = input order) plus streaming
    /// replay cursor. Runs replay before the in-memory tail, preserving
    /// input order end to end.
    pub spilled_runs: Vec<SpilledRun>,
    pub replay_index: usize,
    pub replay_reader: Option<RunReader>,
    pub replay_batch: Option<MaterializedBatch>,
    pub replay_offset: usize,
    /// Memory bytes currently accounted for `batch` rows.
    pub accounted_bytes: usize,
}

#[derive(Debug)]
pub struct DataCollectState {
    /// Column-oriented collected state (was `Vec<Vec<Value>>`).
    pub batch: MaterializedBatch,
    pub emitted: bool,
    /// Output cursor into `batch` (spill path only; the fast path emits one chunk).
    pub emitted_offset: usize,
    pub input_layout: Option<Arc<SlotLayout>>,
    /// Ordered spill runs plus streaming replay cursor (see `MaterializeState`).
    pub spilled_runs: Vec<SpilledRun>,
    pub replay_index: usize,
    pub replay_reader: Option<RunReader>,
    pub replay_batch: Option<MaterializedBatch>,
    pub replay_offset: usize,
    /// Memory bytes currently accounted for `batch` rows.
    pub accounted_bytes: usize,
}

#[derive(Debug)]
pub struct RollUpApplyState {
    /// Column-oriented accumulated state (was `Vec<Vec<Value>>`).
    pub batch: MaterializedBatch,
    /// Output cursor into `batch` (same contract as `MaterializeState`).
    pub emitted_offset: usize,
    /// True once the input has been fully accumulated.
    pub accumulated: bool,
    /// Ordered spill runs plus streaming replay cursor (see `MaterializeState`).
    pub spilled_runs: Vec<SpilledRun>,
    pub replay_index: usize,
    pub replay_reader: Option<RunReader>,
    pub replay_batch: Option<MaterializedBatch>,
    pub replay_offset: usize,
    /// Memory bytes currently accounted for `batch` rows.
    pub accounted_bytes: usize,
}
