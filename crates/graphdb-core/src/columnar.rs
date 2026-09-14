//! Column-oriented materialized state shared by blocking operators.
//!
//! This is the shared columnar abstraction described in the R4 design
//! (`docs/plan/columnar_materialization_state_design.md`). It replaces the
//! row-oriented `Vec<Vec<Value>>` buffers used by `MaterializeState`,
//! `DataCollectState` and `RollUpApplyState` with a column-major layout plus
//! an optional selection vector.
//!
//! Values are stored one `Vec<Value>` per column. This keeps a projected or
//! filtered subset of columns cache-friendly and removes the row<->column
//! transposes that the row-oriented buffers forced at every blocking boundary
//! (see design §2.3). The module lives in `graphdb-core` (the bottom of the
//! DAG) so that both `graphdb-query` and a future `graphdb-storage` columnar
//! rebuild can depend on it without violating the `…→storage→query→…`
//! dependency rule (design §4.2).

use crate::value::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Projected deduplication / grouping key: the key-column subset of a row.
///
/// Stored as owned `Value`s (no new dependencies). A future compact
/// encoding may replace the representation without changing callers.
pub type RowKey = Vec<Value>;

/// FNV-1a 64-bit constants for columnar row hashing.
///
/// Independent copy of the spill partition hash domain parameters so
/// `graphdb-core` does not depend on `graphdb-query`. The seed matches the
/// spill side only by convention; the two domains must not be mixed.
pub const COLUMNAR_HASH_SEED: u64 = 0xdeadbeefcafe;

const FNV1A_64_INIT: u64 = 0xcbf29ce484222325;
const FNV1A_64_PRIME: u64 = 0x100000001b3;

fn fnv1a_64_update(mut hash: u64, data: &[u8]) -> u64 {
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV1A_64_PRIME);
    }
    hash
}

/// Column-name fingerprint: FNV-style hash over column names.
///
/// Mirrors the spill-side `schema_fingerprint` contract without creating a
/// cross-crate dependency. Empty input yields the FNV offset basis.
pub fn column_name_fingerprint(col_names: &[String]) -> u64 {
    let mut hash = FNV1A_64_INIT;
    for name in col_names {
        hash = fnv1a_64_update(hash, name.as_bytes());
        hash = fnv1a_64_update(hash, &[0]);
    }
    hash
}

/// One column buffer: a contiguous `Vec<Value>`.
///
/// Appending a value is O(1) amortized, and the column is stored contiguously
/// so sequential access down a single column is cache-friendly.
#[derive(Debug, Clone, Default)]
pub struct ColumnBuffer {
    /// All values for this logical column, in physical row order.
    pub values: Vec<Value>,
}

impl ColumnBuffer {
    /// Create an empty column buffer.
    pub fn new() -> Self {
        ColumnBuffer { values: Vec::new() }
    }

    /// Create an empty column buffer with capacity for `cap` values.
    pub fn with_capacity(cap: usize) -> Self {
        ColumnBuffer {
            values: Vec::with_capacity(cap),
        }
    }

    /// Append one value to the column.
    pub fn push(&mut self, v: Value) {
        self.values.push(v);
    }

    /// Number of values currently stored.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the column holds no values.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Remove all values.
    pub fn clear(&mut self) {
        self.values.clear();
    }

    /// Borrow the backing slice.
    pub fn as_slice(&self) -> &[Value] {
        &self.values
    }
}

/// Selection vector: logical row `i` maps to physical row `indices[i]`.
///
/// `None` on a [`MaterializedBatch`] encodes the identity selection (logical
/// row `i` <-> physical column index `i`); this is the common case and
/// avoids allocating a vector at all.
#[derive(Debug, Clone, Default)]
pub struct SelectionVector {
    /// Physical row index for each logical row.
    pub indices: Vec<usize>,
}

impl SelectionVector {
    /// Create an empty (non-identity) selection vector.
    pub fn new() -> Self {
        SelectionVector {
            indices: Vec::new(),
        }
    }

    /// Create an identity selection of length `len` (`indices[i] == i`).
    pub fn identity(len: usize) -> Self {
        SelectionVector {
            indices: (0..len).collect(),
        }
    }

    /// Whether this selection is the identity mapping.
    pub fn is_identity(&self) -> bool {
        self.indices.iter().enumerate().all(|(i, &p)| i == p)
    }

    /// Number of logical rows covered.
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Whether the selection is empty.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Column-oriented materialized batch.
///
/// Replaces `Vec<Vec<Value>>` in `MaterializeState` / `DataCollectState` /
/// `RollUpApplyState`. Logical rows are addressed through `num_rows()` which
/// honors an optional [`SelectionVector`].
#[derive(Debug, Clone)]
pub struct MaterializedBatch {
    /// One buffer per output column, in schema order.
    columns: Vec<ColumnBuffer>,
    /// Physical row count (length of every column buffer).
    num_rows: usize,
    /// Optional selection vector; `None` == identity over `0..num_rows`.
    selection: Option<SelectionVector>,
    /// Schema fingerprint for spill-compatibility checks (design §3.4).
    /// Until R3 lands, this is the hashed column count; the column-name
    /// fingerprint from `spill.rs` will be threaded through when columnar
    /// run files are added.
    schema_fingerprint: u64,
    /// Optional column names backing the fingerprint. `None` preserves the
    /// legacy column-count-only fingerprint behavior.
    schema_names: Option<Arc<[String]>>,
}

impl MaterializedBatch {
    /// Create an empty batch with `num_columns` columns.
    pub fn new(num_columns: usize, schema_fingerprint: u64) -> Self {
        MaterializedBatch {
            columns: (0..num_columns).map(|_| ColumnBuffer::new()).collect(),
            num_rows: 0,
            selection: None,
            schema_fingerprint,
            schema_names: None,
        }
    }

    /// Create an empty batch with column names backing the fingerprint.
    ///
    /// The fingerprint is derived from the column names so batches with the
    /// same column count but different schemas no longer collide. When
    /// `names` is empty the legacy column-count fingerprint applies.
    pub fn with_schema_names(names: Arc<[String]>) -> Self {
        let fingerprint = column_name_fingerprint(&names);
        MaterializedBatch {
            columns: (0..names.len()).map(|_| ColumnBuffer::new()).collect(),
            num_rows: 0,
            selection: None,
            schema_fingerprint: fingerprint,
            schema_names: Some(names),
        }
    }

    /// Attach column names and recompute the fingerprint from them.
    pub fn set_schema_names(&mut self, names: Arc<[String]>) {
        self.schema_fingerprint = column_name_fingerprint(&names);
        self.schema_names = Some(names);
    }

    /// Borrow the column names backing the fingerprint, if any.
    pub fn schema_names(&self) -> Option<&Arc<[String]>> {
        self.schema_names.as_ref()
    }

    /// Override the spill-compatibility token (used by run replay checks).
    pub fn set_fingerprint(&mut self, fingerprint: u64) {
        self.schema_fingerprint = fingerprint;
    }

    /// Build a batch from an iterator of rows.
    ///
    /// The column count is inferred from the first row; all subsequent rows
    /// must have the same arity. The schema fingerprint is the hashed column
    /// count (placeholder until R3 threads the column-name fingerprint).
    pub fn from_rows(rows: impl IntoIterator<Item = Vec<Value>>) -> Self {
        let mut batch = MaterializedBatch::new(0, 0);
        for row in rows {
            batch.append_row(row);
        }
        batch.refresh_fingerprint();
        batch
    }

    /// Number of columns in the batch.
    pub fn num_columns(&self) -> usize {
        self.columns.len()
    }

    /// Logical row count (length of the selection, or physical count).
    pub fn num_rows(&self) -> usize {
        match &self.selection {
            Some(sel) => sel.indices.len(),
            None => self.num_rows,
        }
    }

    /// Whether the batch holds no logical rows.
    pub fn is_empty(&self) -> bool {
        self.num_rows() == 0
    }

    /// Schema fingerprint (spill-compatibility token).
    pub fn schema_fingerprint(&self) -> u64 {
        self.schema_fingerprint
    }

    /// Append one row, growing the column set from the first row's arity.
    ///
    /// Panics if a non-empty batch receives a row with a different column
    /// count; blocking operators feed rows of uniform arity so this invariant
    /// always holds.
    pub fn append_row(&mut self, row: Vec<Value>) {
        if self.columns.is_empty() {
            self.columns = (0..row.len()).map(|_| ColumnBuffer::new()).collect();
        }
        assert_eq!(
            row.len(),
            self.columns.len(),
            "MaterializedBatch: row arity {} != column count {}",
            row.len(),
            self.columns.len()
        );
        for (col, v) in row.into_iter().enumerate() {
            self.columns[col].push(v);
        }
        self.num_rows += 1;
    }

    /// Append one pre-built column of values.
    ///
    /// The column length must match the existing row count (or define it
    /// when the batch is empty). Column count grows by one.
    pub fn append_column(&mut self, values: Vec<Value>) {
        if self.columns.is_empty() {
            self.columns.push(ColumnBuffer { values });
            self.num_rows = self.columns[0].len();
            return;
        }
        assert_eq!(
            values.len(),
            self.num_rows,
            "MaterializedBatch: column length {} != row count {}",
            values.len(),
            self.num_rows
        );
        self.columns.push(ColumnBuffer { values });
    }

    /// Borrow column `i`.
    pub fn column(&self, i: usize) -> &ColumnBuffer {
        &self.columns[i]
    }

    /// Logical row `i` (after selection) as an owned `Vec<Value>`.
    pub fn row(&self, i: usize) -> Vec<Value> {
        match &self.selection {
            Some(sel) => {
                let phys = sel.indices[i];
                self.columns
                    .iter()
                    .map(|c| c.values[phys].clone())
                    .collect()
            }
            None => self.columns.iter().map(|c| c.values[i].clone()).collect(),
        }
    }

    /// Iterate logical rows as owned `Vec<Value>`.
    pub fn rows(&self) -> impl Iterator<Item = Vec<Value>> + '_ {
        let n = self.num_rows();
        (0..n).map(move |i| self.row(i))
    }

    /// All logical rows as owned `Vec<Vec<Value>>` (borrow version).
    pub fn to_rows(&self) -> Vec<Vec<Value>> {
        self.rows().collect()
    }

    /// Drain the batch's column buffers and return all logical rows without
    /// cloning values (transposes column buffers into row vectors). The batch is
    /// left empty but remains a valid zero-row state, so it can be called
    /// through a `&mut` reference owned by an operator state struct.
    pub fn into_rows(&mut self) -> Vec<Vec<Value>> {
        let logical = self.num_rows();
        let columns = std::mem::take(&mut self.columns);
        let mut rows = vec![Vec::with_capacity(columns.len()); logical];
        match &self.selection {
            Some(sel) => {
                for (logical_idx, &phys) in sel.indices.iter().enumerate() {
                    for col in columns.iter() {
                        rows[logical_idx].push(col.values[phys].clone());
                    }
                }
            }
            None => {
                for col in columns.into_iter() {
                    for (r, v) in col.values.into_iter().enumerate() {
                        rows[r].push(v);
                    }
                }
            }
        }
        self.num_rows = 0;
        rows
    }

    /// Approximate in-memory footprint in bytes, for `MemoryTracker` accounting.
    ///
    /// This is a conservative per-value heuristic plus per-column vector
    /// overhead; it intentionally does not try to measure heap payloads behind
    /// `Value`'s indirection (the row-oriented path used the same heuristic via
    /// `MemoryBudget::estimate_row_memory`).
    pub fn memory_size(&self) -> usize {
        let values: usize = self.columns.iter().map(|c| c.values.len()).sum();
        values * std::mem::size_of::<Value>() + self.columns.len() * 24
    }

    /// Install a selection vector over the physical rows.
    ///
    /// The selection length must equal the logical row count (i.e. the value
    /// returned by `num_rows()` before installing it).
    pub fn set_selection(&mut self, sel: SelectionVector) {
        self.selection = Some(sel);
    }

    /// Clear any installed selection vector, reverting to identity.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Recompute the schema fingerprint from the current column count.
    ///
    /// Legacy behavior preserved for batches without column names. Prefer
    /// [`Self::refresh_fingerprint_from_names`] for new code.
    pub fn refresh_fingerprint(&mut self) {
        if let Some(names) = self.schema_names.clone() {
            self.schema_fingerprint = column_name_fingerprint(&names);
            return;
        }
        let mut h = DefaultHasher::new();
        self.columns.len().hash(&mut h);
        self.schema_fingerprint = h.finish();
    }

    /// Recompute the fingerprint from explicit column names.
    pub fn refresh_fingerprint_from_names(&mut self, names: &[String]) {
        self.schema_fingerprint = column_name_fingerprint(names);
    }

    /// Remove all rows while keeping the column layout and names.
    pub fn clear(&mut self) {
        for col in &mut self.columns {
            col.clear();
        }
        self.num_rows = 0;
        self.selection = None;
    }

    /// Borrow one column as a value slice.
    pub fn column_slice(&self, i: usize) -> &[Value] {
        self.columns[i].as_slice()
    }

    /// Copy a row range into a new batch (clamped to the logical size).
    ///
    /// The selection, if any, is resolved so the output batch is always
    /// identity-selected. Schema names are carried over.
    pub fn slice_rows(&self, offset: usize, len: usize) -> MaterializedBatch {
        let total = self.num_rows();
        if offset >= total || len == 0 {
            let mut out = MaterializedBatch::new(self.columns.len(), self.schema_fingerprint);
            out.schema_names = self.schema_names.clone();
            return out;
        }
        let end = (offset + len).min(total);
        let mut out = MaterializedBatch::new(0, self.schema_fingerprint);
        out.schema_names = self.schema_names.clone();
        out.columns = self.columns.iter().map(|_| ColumnBuffer::new()).collect();
        match &self.selection {
            Some(sel) => {
                for logical in offset..end {
                    let phys = sel.indices[logical];
                    for (dst, src) in out.columns.iter_mut().zip(self.columns.iter()) {
                        dst.push(src.values[phys].clone());
                    }
                }
            }
            None => {
                for (dst, src) in out.columns.iter_mut().zip(self.columns.iter()) {
                    dst.values.extend_from_slice(&src.values[offset..end]);
                    // `extend_from_slice` clones each `Value`; explicit loop
                    // avoided for clarity, semantics identical.
                }
            }
        }
        out.num_rows = end - offset;
        out
    }

    /// Reorder/copy rows by physical indices into a new identity batch.
    pub fn gather(&self, indices: &[usize]) -> MaterializedBatch {
        let mut out = MaterializedBatch::new(0, self.schema_fingerprint);
        out.schema_names = self.schema_names.clone();
        out.columns = self.columns.iter().map(|_| ColumnBuffer::new()).collect();
        // Resolve through the installed selection so callers pass logical
        // positions uniformly.
        for &logical in indices {
            let phys = match &self.selection {
                Some(sel) => sel.indices[logical],
                None => logical,
            };
            for (dst, src) in out.columns.iter_mut().zip(self.columns.iter()) {
                dst.push(src.values[phys].clone());
            }
        }
        out.num_rows = indices.len();
        out
    }

    /// Hash each logical row over a column subset (FNV-1a over postcard bytes).
    ///
    /// `cols` empty means all columns. Only the equality partition matters:
    /// rows equal under `Value` equality share a hash; unequal rows may (rarely)
    /// collide. Null/NaN/mixed-kind handling follows postcard encoding, and
    /// equivalence with `HashSet<Vec<Value>>` is covered by tests.
    pub fn hash_rows(&self, cols: &[usize]) -> Vec<u64> {
        let n = self.num_rows();
        let mut out = Vec::with_capacity(n);
        let use_all = cols.is_empty();
        for logical in 0..n {
            let phys = match &self.selection {
                Some(sel) => sel.indices[logical],
                None => logical,
            };
            let mut hash = COLUMNAR_HASH_SEED;
            if use_all {
                for col in &self.columns {
                    let bytes = postcard::to_allocvec(&col.values[phys]).unwrap_or_default();
                    hash = fnv1a_64_update(hash, &bytes);
                    hash = fnv1a_64_update(hash, &[0xff]);
                }
            } else {
                for &c in cols {
                    let bytes =
                        postcard::to_allocvec(&self.columns[c].values[phys]).unwrap_or_default();
                    hash = fnv1a_64_update(hash, &bytes);
                    hash = fnv1a_64_update(hash, &[0xff]);
                }
            }
            out.push(hash);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    fn row_i64(v: i64) -> Vec<Value> {
        vec![Value::BigInt(v), Value::string(format!("k{v}"))]
    }

    #[test]
    fn row_round_trip_preserves_order_and_arity() {
        let rows = vec![row_i64(1), row_i64(2), row_i64(3)];
        let batch = MaterializedBatch::from_rows(rows.clone());
        assert_eq!(batch.num_rows(), 3);
        assert_eq!(batch.num_columns(), 2);
        assert_eq!(batch.to_rows(), rows);
    }

    #[test]
    fn column_major_layout_is_contiguous() {
        let batch = MaterializedBatch::from_rows(vec![row_i64(1), row_i64(2)]);
        let c0 = batch.column(0).as_slice();
        assert_eq!(c0, &[Value::BigInt(1), Value::BigInt(2)]);
        let c1 = batch.column(1).as_slice();
        assert_eq!(c1, &[Value::string("k1"), Value::string("k2")]);
    }

    #[test]
    fn into_rows_moves_without_clone_and_preserves_values() {
        let rows = vec![row_i64(10), row_i64(20)];
        let mut batch = MaterializedBatch::from_rows(rows.clone());
        assert_eq!(batch.into_rows(), rows);
    }

    #[test]
    fn empty_batch_is_empty() {
        let batch = MaterializedBatch::new(2, 0);
        assert!(batch.is_empty());
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.to_rows(), Vec::<Vec<Value>>::new());
    }

    #[test]
    fn selection_vector_reorders_logical_rows() {
        let batch = MaterializedBatch::from_rows(vec![row_i64(1), row_i64(2), row_i64(3)]);
        let mut batch = batch;
        batch.set_selection(SelectionVector {
            indices: vec![2, 0],
        });
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.row(0), row_i64(3));
        assert_eq!(batch.row(1), row_i64(1));
        // into_rows honors the selection
        let out = batch.into_rows();
        assert_eq!(out, vec![row_i64(3), row_i64(1)]);
    }

    #[test]
    fn memory_size_is_positive() {
        let batch = MaterializedBatch::from_rows(vec![row_i64(1), row_i64(2)]);
        assert!(batch.memory_size() > 0);
    }

    #[test]
    fn fingerprint_is_column_name_sensitive() {
        let a: Arc<[String]> = Arc::from(vec!["id".to_string(), "name".to_string()]);
        let b: Arc<[String]> = Arc::from(vec!["id".to_string(), "other".to_string()]);
        let ba = MaterializedBatch::with_schema_names(Arc::clone(&a));
        let bb = MaterializedBatch::with_schema_names(Arc::clone(&b));
        assert_ne!(ba.schema_fingerprint(), bb.schema_fingerprint());
        assert_eq!(ba.schema_fingerprint(), column_name_fingerprint(&a));

        // Same column count, different names: legacy count-only hashing
        // would collide, name-based must not.
        let mut legacy = MaterializedBatch::from_rows(vec![row_i64(1)]);
        legacy.refresh_fingerprint();
        let mut named = MaterializedBatch::from_rows(vec![row_i64(1)]);
        named.set_schema_names(Arc::from(vec!["x".to_string(), "y".to_string()]));
        named.refresh_fingerprint();
        assert_eq!(
            named.schema_fingerprint(),
            column_name_fingerprint(&["x".to_string(), "y".to_string()])
        );
    }

    #[test]
    fn slice_and_gather_respect_selection() {
        let mut batch = MaterializedBatch::from_rows(vec![row_i64(1), row_i64(2), row_i64(3)]);
        batch.set_selection(SelectionVector {
            indices: vec![2, 0, 1],
        });
        let sliced = batch.slice_rows(1, 5);
        assert_eq!(sliced.to_rows(), vec![row_i64(1), row_i64(2)]);
        let gathered = batch.gather(&[0, 0, 2]);
        assert_eq!(gathered.to_rows(), vec![row_i64(3), row_i64(3), row_i64(2)]);
        let empty = batch.slice_rows(10, 3);
        assert!(empty.is_empty());
    }

    #[test]
    fn append_column_extends_schema() {
        let mut batch = MaterializedBatch::new(0, 0);
        batch.append_column(vec![Value::BigInt(1), Value::BigInt(2)]);
        batch.append_column(vec![Value::string("a"), Value::string("b")]);
        assert_eq!(batch.num_columns(), 2);
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(
            batch.to_rows(),
            vec![
                vec![Value::BigInt(1), Value::string("a")],
                vec![Value::BigInt(2), Value::string("b")],
            ]
        );
    }

    #[test]
    fn hash_rows_partition_matches_row_set_equality() {
        use crate::value::NullType;
        use std::collections::HashSet;
        let rows = vec![
            vec![Value::BigInt(1), Value::string("k")],
            vec![Value::BigInt(1), Value::string("k")],
            vec![Value::BigInt(2), Value::string("k")],
            vec![Value::Null(NullType::Null), Value::string("k")],
            vec![Value::Null(NullType::Null), Value::string("k")],
            vec![Value::Double(f64::NAN), Value::Int(1)],
            vec![Value::Int(1), Value::BigInt(1)],
        ];
        let batch = MaterializedBatch::from_rows(rows.clone());
        let hashes = batch.hash_rows(&[]);
        // Equal rows share a hash.
        assert_eq!(hashes[0], hashes[1]);
        assert_eq!(hashes[3], hashes[4]);
        // Distinct rows (new test): at least one differs; NaN rows hash
        // deterministically (same bit pattern -> same hash).
        assert_ne!(hashes[0], hashes[2]);
        let dup = MaterializedBatch::from_rows(vec![vec![Value::Double(f64::NAN), Value::Int(1)]]);
        assert_eq!(dup.hash_rows(&[])[0], hashes[5]);
        // Column-subset hashing: rows differing only outside `cols` collide.
        let sub = batch.hash_rows(&[0]);
        assert_eq!(sub[0], sub[1]);
        // Reference partition check: HashSet equality classes are respected
        // (equal HashSet members always share a hash here).
        let mut set: HashSet<Vec<Value>> = HashSet::new();
        for (i, row) in rows.iter().enumerate() {
            if !set.contains(row) {
                set.insert(row.clone());
                for (j, other) in rows.iter().enumerate() {
                    if row == other {
                        assert_eq!(hashes[i], hashes[j], "equal rows must share hash");
                    }
                }
            }
        }
    }
}
