//! DataChunk: Basic unit of streaming execution
//!
//! A DataChunk represents a fixed-size batch of rows processed in streaming mode.
//! Typical size: 1024 rows (~4MB)
//!
//! # Ownership & Memory Accounting Rules (M4)
//!
//! - **`rows`**: Owned `Vec<Vec<Value>>`. Deep-cloned on `Clone` (transitional).
//! - **`schema`**: `Arc<Schema>` — shared reference, cheap `Arc::clone` on `Clone`.
//! - **`layout`**: `Arc<SlotLayout>` — always present; shared reference, cheap on `Clone`.
//! - **`memory_reservation`**: `Option<MemoryReservation>` / `Option<MemoryPoolReservation>` —
//!   ownership stays with the original chunk on `Clone`; the clone gets `None`.
//!   Use `take_memory_reservation()` to transfer ownership explicitly.
//!
//! ## Clone removal (M4)
//!
//! `Clone` is provided for migration only.  New code should use one of:
//! - **`deep_copy(pool)`**: creates a new chunk with rows deep-copied into the
//!   given memory pool.  Properly accounts the new memory.
//! - **`view()`**: creates a zero-copy [`ChunkView`] borrowing the parent chunk.
//! - **`slice(range)`**: moves a subset of rows into a new chunk (efficient,
//!   uses `std::mem::take` per row).
//!
//! # Construction Paths
//!
//! - `new_with_layout(rows, layout)` — **Production path**. Schema is derived from
//!   layout slot metadata. Always preferred when a `SlotLayout` is available.
//! - `new(rows, schema)` — Schema-driven path. Layout auto-created from column names.
//! - `from_rows(rows)` / `from_rows_with_col_names(rows, col_names)` — Convenience
//!   constructors for tests and legacy code. Always produce a layout (auto-created).

use super::runtime::ColumnarStats;
use super::slot::{SlotId, SlotLayout};
use crate::core::types::expr::Expression;
use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::Value;
use crate::query::executor::base::MemoryReservation;
use crate::query::executor::expression::evaluator::operations::{
    BinaryOperationEvaluator, UnaryOperationEvaluator,
};
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::expression::ExpressionError;
use crate::query::executor::streaming::context::BorrowedRowContext;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

const ROW_POOL_MAX_SIZE: usize = 8;

/// Runtime switch: typed column layout for produced chunks.
///
/// Rollback knob — set to `false` to fall back to the row-major path for
/// every chunk (no typed columns are built, `get_column` / `eval_with_cache`
/// behave exactly as before the typed layout existed).
static TYPED_COLUMNS_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable or disable the typed column layout (rollback switch).
pub fn set_typed_columns_enabled(enabled: bool) {
    TYPED_COLUMNS_ENABLED.store(enabled, AtomicOrdering::Relaxed);
}

/// Whether the typed column layout is currently enabled.
pub fn typed_columns_enabled() -> bool {
    TYPED_COLUMNS_ENABLED.load(AtomicOrdering::Relaxed)
}

/// Runtime switch: selection-vector propagation across operators.
///
/// Rollback knob — set to `false` to make `Filter` materialise selected rows
/// via `take_indices` exactly as before selection propagation existed.
static SELECTION_PROPAGATION_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable or disable selection-vector propagation (rollback switch).
pub fn set_selection_propagation_enabled(enabled: bool) {
    SELECTION_PROPAGATION_ENABLED.store(enabled, AtomicOrdering::Relaxed);
}

/// Whether selection-vector propagation is currently enabled.
pub fn selection_propagation_enabled() -> bool {
    SELECTION_PROPAGATION_ENABLED.load(AtomicOrdering::Relaxed)
}

/// Kind of a typed fixed-size scalar column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedKind {
    I64,
    F64,
    I32,
}

/// Typed column representation for fixed-size scalar columns.
///
/// `I64`/`F64`/`I32` columns are stored as dense raw `Vec`s so that batch
/// evaluation operates on scalars (auto-vectorizable) instead of constructing
/// one `Value` per row. Columns that contain NULLs, mixed types, or
/// non-scalar values fall back to [`TypedColumn::Fallback`].
#[derive(Debug, Clone)]
pub enum TypedColumn {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Fallback(Vec<Value>),
}

impl TypedColumn {
    pub fn len(&self) -> usize {
        match self {
            TypedColumn::I64(v) => v.len(),
            TypedColumn::F64(v) => v.len(),
            TypedColumn::I32(v) => v.len(),
            TypedColumn::Fallback(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether this column uses a typed (non-fallback) representation.
    pub fn is_typed(&self) -> bool {
        !matches!(self, TypedColumn::Fallback(_))
    }

    /// Materialize the value at `idx` (O(1) for typed variants).
    pub fn value_at(&self, idx: usize) -> Option<Value> {
        match self {
            TypedColumn::I64(v) => v.get(idx).map(|&x| Value::BigInt(x)),
            TypedColumn::F64(v) => v.get(idx).map(|&x| Value::Double(x)),
            TypedColumn::I32(v) => v.get(idx).map(|&x| Value::Int(x)),
            TypedColumn::Fallback(v) => v.get(idx).cloned(),
        }
    }

    /// Convert the whole column into `Vec<Value>`.
    pub fn to_values(&self) -> Vec<Value> {
        match self {
            TypedColumn::I64(v) => v.iter().map(|&x| Value::BigInt(x)).collect(),
            TypedColumn::F64(v) => v.iter().map(|&x| Value::Double(x)).collect(),
            TypedColumn::I32(v) => v.iter().map(|&x| Value::Int(x)).collect(),
            TypedColumn::Fallback(v) => v.clone(),
        }
    }

    /// Estimated heap bytes of this column (for memory accounting).
    pub fn estimated_size(&self) -> usize {
        match self {
            TypedColumn::I64(v) => v.capacity() * std::mem::size_of::<i64>(),
            TypedColumn::F64(v) => v.capacity() * std::mem::size_of::<f64>(),
            TypedColumn::I32(v) => v.capacity() * std::mem::size_of::<i32>(),
            TypedColumn::Fallback(v) => v.iter().map(Value::estimated_size).sum(),
        }
    }
}

/// Pool of recycled `Vec<Vec<Value>>` allocations for DataChunk construction.
///
/// Reduces allocation overhead by reusing Vec buffers across chunk boundaries.
/// Each acquired Vec is guaranteed to have `chunk_size` capacity (not length).
/// Typed allocation pools (`Vec<i64>`/`Vec<f64>`/`Vec<i32>`) recycle
/// typed column buffers for `TypedColumn` construction.
pub struct RowPool {
    pool: parking_lot::Mutex<Vec<Vec<Vec<Value>>>>,
    typed_i64: parking_lot::Mutex<Vec<Vec<i64>>>,
    typed_f64: parking_lot::Mutex<Vec<Vec<f64>>>,
    typed_i32: parking_lot::Mutex<Vec<Vec<i32>>>,
    chunk_size: usize,
    num_columns: usize,
}

impl RowPool {
    pub fn new(chunk_size: usize, num_columns: usize) -> Self {
        Self {
            pool: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
            typed_i64: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
            typed_f64: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
            typed_i32: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
            chunk_size,
            num_columns,
        }
    }

    /// Acquire a pre-allocated rows buffer from the pool, or create a new one.
    pub fn acquire(&self) -> Vec<Vec<Value>> {
        let mut pool = self.pool.lock();
        if let Some(mut rows) = pool.pop() {
            rows.clear();
            rows
        } else {
            Vec::with_capacity(self.chunk_size)
        }
    }

    /// Return a rows buffer to the pool for reuse.
    /// The buffer is cleared and made available for future `acquire()` calls.
    pub fn release(&self, mut rows: Vec<Vec<Value>>) {
        let mut pool = self.pool.lock();
        if pool.len() < ROW_POOL_MAX_SIZE {
            for row in &mut rows {
                row.clear();
            }
            rows.clear();
            pool.push(rows);
        }
    }

    /// Acquire a pre-allocated typed column buffer of the given kind.
    pub fn acquire_typed(&self, kind: TypedKind) -> TypedColumn {
        let cap = self.chunk_size;
        match kind {
            TypedKind::I64 => {
                let mut p = self.typed_i64.lock();
                if let Some(mut buf) = p.pop() {
                    buf.clear();
                    TypedColumn::I64(buf)
                } else {
                    TypedColumn::I64(Vec::with_capacity(cap))
                }
            }
            TypedKind::F64 => {
                let mut p = self.typed_f64.lock();
                if let Some(mut buf) = p.pop() {
                    buf.clear();
                    TypedColumn::F64(buf)
                } else {
                    TypedColumn::F64(Vec::with_capacity(cap))
                }
            }
            TypedKind::I32 => {
                let mut p = self.typed_i32.lock();
                if let Some(mut buf) = p.pop() {
                    buf.clear();
                    TypedColumn::I32(buf)
                } else {
                    TypedColumn::I32(Vec::with_capacity(cap))
                }
            }
        }
    }

    /// Return a typed column buffer to the pool for reuse.
    /// Fallback columns are discarded (they wrap `Vec<Value>`).
    pub fn release_typed(&self, column: TypedColumn) {
        match column {
            TypedColumn::I64(mut buf) => {
                buf.clear();
                let mut p = self.typed_i64.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::F64(mut buf) => {
                buf.clear();
                let mut p = self.typed_f64.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::I32(mut buf) => {
                buf.clear();
                let mut p = self.typed_i32.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::Fallback(_) => {}
        }
    }

    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    pub fn num_columns(&self) -> usize {
        self.num_columns
    }
}

impl std::fmt::Debug for RowPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowPool")
            .field("chunk_size", &self.chunk_size)
            .field("num_columns", &self.num_columns)
            .finish()
    }
}

/// A chunk of rows processed in streaming execution
#[derive(Debug)]
pub struct DataChunk {
    /// Row data with Value types
    pub rows: Vec<Vec<Value>>,
    /// Optional column-major representation for efficient columnar access.
    /// When populated, each inner Vec holds all values for one column.
    /// This enables O(1) column extraction without per-row cloning.
    pub columns: Option<Vec<Vec<Value>>>,
    /// Optional typed column layout. When populated, one entry per slot:
    /// fixed-size scalar columns (BigInt/Double/Int) are stored as raw
    /// `Vec<i64>`/`Vec<f64>`/`Vec<i32>`; everything else is `Fallback`.
    /// Built eagerly by source operators, gathered by `take_indices`/`slice`,
    /// and consumed by the typed batch evaluator. Length of every column
    /// equals `rows.len()`.
    pub typed_columns: Option<Vec<TypedColumn>>,
    /// Selection vector. When `Some(indices)`, only rows at those indices
    /// are "visible" to downstream consumers; `None` means all rows are
    /// visible. Indices are sorted and unique. Consumers that cannot honour
    /// a selection must call `materialize_selection()` first.
    pub selection: Option<Vec<usize>>,
    /// Schema information (column names and types)
    pub schema: Arc<Schema>,
    /// Slot layout for slot-based value access.
    /// Always set for production chunks; convenience constructors auto-create
    /// from column names when no explicit layout is provided.
    pub layout: Arc<SlotLayout>,
    /// Memory reservation for this chunk's data.
    /// Dropping the chunk releases the reserved bytes.
    pub memory_reservation: Option<MemoryReservation>,
    /// Query-level columnar fast-path counters (observability).
    /// Shared per query; `None` for chunks that were not produced by a source
    /// operator or whose producer had no runtime attached.
    pub columnar_stats: Option<Arc<ColumnarStats>>,
}

// M4 transitional: Clone loses memory accounting.
// Use `deep_copy(pool)` or `slice()` instead of clone for new code.
impl Clone for DataChunk {
    fn clone(&self) -> Self {
        Self {
            rows: self.rows.clone(),
            columns: self.columns.clone(),
            typed_columns: self.typed_columns.clone(),
            selection: self.selection.clone(),
            schema: self.schema.clone(),
            layout: Arc::clone(&self.layout),
            memory_reservation: None,
            columnar_stats: self.columnar_stats.clone(),
        }
    }
}

/// Simple schema representation
#[derive(Debug, Clone)]
pub struct Schema {
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    /// Column data type (inferred from values if not specified)
    pub data_type: String,
}

impl Schema {
    pub fn new(columns: Vec<ColumnInfo>) -> Self {
        Self { columns }
    }

    pub fn empty() -> Self {
        Self { columns: vec![] }
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
}

impl DataChunk {
    /// Create a new DataChunk with rows and schema.
    /// SlotLayout is auto-created from schema column names.
    pub fn new(rows: Vec<Vec<Value>>, schema: Arc<Schema>) -> Self {
        let layout = Arc::new(SlotLayout::from_names(
            &schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>(),
        ));
        Self {
            rows,
            columns: None,
            typed_columns: None,
            selection: None,
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        }
    }

    /// Attach a memory reservation to this chunk.
    /// The reserved bytes are released when the chunk is dropped.
    pub fn with_memory_reservation(mut self, reservation: MemoryReservation) -> Self {
        self.memory_reservation = Some(reservation);
        self
    }

    /// Attach query-level columnar fast-path counters (observability).
    ///
    /// Source operators attach the runtime's counters so that Filter/Project
    /// evaluation on this chunk can expose the columnar hit/miss rate.
    pub fn with_columnar_stats(mut self, stats: Arc<ColumnarStats>) -> Self {
        self.columnar_stats = Some(stats);
        self
    }

    /// Consume the memory reservation, leaving `None` in its place.
    /// The caller becomes responsible for releasing the reserved memory.
    pub fn take_memory_reservation(&mut self) -> Option<MemoryReservation> {
        self.memory_reservation.take()
    }

    /// Create a DataChunk with explicit SlotLayout.
    /// Schema is derived from the layout's slot metadata.
    ///
    /// Panics if row width does not match layout width.
    pub fn new_with_layout(rows: Vec<Vec<Value>>, layout: Arc<SlotLayout>) -> Self {
        Self::try_new_with_layout(rows, layout).expect("DataChunk row width mismatch")
    }

    /// Fallible variant of [`new_with_layout`](Self::new_with_layout).
    ///
    /// Returns `Err` when row width does not match layout width, instead of
    /// panicking. Use this in production paths where the invariant is not
    /// structurally guaranteed.
    pub fn try_new_with_layout(
        rows: Vec<Vec<Value>>,
        layout: Arc<SlotLayout>,
    ) -> Result<Self, crate::core::error::QueryError> {
        let row_width = rows.first().map(Vec::len).unwrap_or(0);
        if !layout.is_empty()
            && !rows.is_empty()
            && !rows.iter().all(|row| row.len() == layout.len())
        {
            return Err(crate::core::error::QueryError::execution(format!(
                "DataChunk::new_with_layout: row width {} does not match layout width {}",
                row_width,
                layout.len()
            )));
        }
        let columns: Vec<ColumnInfo> = layout
            .slots
            .iter()
            .map(|info| ColumnInfo {
                name: info.name.clone(),
                data_type: info
                    .data_type
                    .as_ref()
                    .map(|dt| dt.to_string().to_lowercase())
                    .unwrap_or_else(|| "unknown".to_string()),
            })
            .collect();
        let schema = Arc::new(Schema::new(columns));
        Ok(Self {
            rows,
            columns: None,
            typed_columns: None,
            selection: None,
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        })
    }

    /// Create a DataChunk from rows, inferring schema and generating col_N names.
    /// SlotLayout is auto-created from the inferred column names.
    pub fn from_rows(rows: Vec<Vec<Value>>) -> Self {
        Self::from_rows_with_col_names(rows, None)
    }

    /// Create a DataChunk from rows, using provided column names if available.
    ///
    /// When col_names is None, falls back to col_N inference (backward compat).
    /// When col_names is Some, uses those names directly.
    /// SlotLayout is auto-created from the resulting column names.
    pub fn from_rows_with_col_names(rows: Vec<Vec<Value>>, col_names: Option<Vec<String>>) -> Self {
        let schema = if rows.is_empty() {
            if let Some(names) = col_names {
                Arc::new(Schema::new(
                    names
                        .into_iter()
                        .map(|name| ColumnInfo {
                            name,
                            data_type: "unknown".to_string(),
                        })
                        .collect(),
                ))
            } else {
                Arc::new(Schema::empty())
            }
        } else {
            let col_count = rows[0].len();
            let columns = (0..col_count)
                .map(|i| {
                    let name = col_names
                        .as_ref()
                        .and_then(|names| names.get(i).cloned())
                        .unwrap_or_else(|| format!("col_{}", i));

                    let data_type = if let Some(row) = rows.first() {
                        if let Some(val) = row.get(i) {
                            match val {
                                Value::BigInt(_) => "bigint",
                                Value::Int(_) => "int",
                                Value::Double(_) => "double",
                                Value::Float(_) => "float",
                                Value::String(_) => "string",
                                Value::Bool(_) => "bool",
                                Value::Null(_) => "null",
                                _ => "unknown",
                            }
                        } else {
                            "unknown"
                        }
                    } else {
                        "unknown"
                    };

                    ColumnInfo {
                        name,
                        data_type: data_type.to_string(),
                    }
                })
                .collect();
            Arc::new(Schema::new(columns))
        };
        let layout = Arc::new(SlotLayout::from_names(
            &schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>(),
        ));
        Self {
            rows,
            columns: None,
            typed_columns: None,
            selection: None,
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        }
    }

    /// Create a DataChunk from column-major data.
    ///
    /// Builds rows by transposing columns, and stores the columnar
    /// representation directly (avoiding a separate materialization pass).
    /// This is more efficient when the caller already has column-major data.
    ///
    /// Panics if column lengths are inconsistent or layout width doesn't
    /// match the number of columns.
    pub fn from_columns(columns: Vec<Vec<Value>>, layout: Arc<SlotLayout>) -> Self {
        let num_cols = columns.len();
        assert!(
            layout.is_empty() || num_cols == layout.len(),
            "DataChunk::from_columns: column count {} does not match layout width {}",
            num_cols,
            layout.len()
        );
        let num_rows = columns.first().map(|c| c.len()).unwrap_or(0);
        assert!(
            columns.iter().all(|c| c.len() == num_rows),
            "DataChunk::from_columns: column length mismatch"
        );

        let mut rows = vec![Vec::with_capacity(num_cols); num_rows];
        for col in columns.iter().take(num_cols) {
            for (row_idx, val) in col.iter().enumerate().take(num_rows) {
                rows[row_idx].push(val.clone());
            }
        }

        let schema = Arc::new(Schema::new(
            layout
                .slots
                .iter()
                .map(|info| ColumnInfo {
                    name: info.name.clone(),
                    data_type: info
                        .data_type
                        .as_ref()
                        .map(|dt| dt.to_string().to_lowercase())
                        .unwrap_or_else(|| "unknown".to_string()),
                })
                .collect(),
        ));

        Self {
            rows,
            columns: Some(columns),
            typed_columns: None,
            selection: None,
            schema,
            layout,
            memory_reservation: None,
            columnar_stats: None,
        }
    }

    /// Attach columnar data to this chunk.
    ///
    /// The columns Vec must have one inner Vec per column, each with length
    /// equal to the number of rows. When columns are set, `get_column()` uses
    /// them directly without cloning from rows.
    ///
    /// Panics if the column count or row counts don't match.
    pub fn with_columns(mut self, columns: Vec<Vec<Value>>) -> Self {
        assert_eq!(columns.len(), self.num_columns(), "column count mismatch");
        if !self.rows.is_empty() {
            for col in &columns {
                assert_eq!(col.len(), self.len(), "column length mismatch");
            }
        }
        self.columns = Some(columns);
        self
    }

    /// Number of rows in this chunk
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether this chunk is empty
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Number of columns
    pub fn num_columns(&self) -> usize {
        self.schema.column_count()
    }

    /// Get column names from schema
    pub fn col_names(&self) -> Vec<String> {
        self.schema.columns.iter().map(|c| c.name.clone()).collect()
    }

    /// Get column name by index
    pub fn col_name(&self, index: usize) -> Option<String> {
        self.schema.columns.get(index).map(|c| c.name.clone())
    }

    /// Create column name to index mapping
    pub fn col_name_index(&self) -> std::collections::HashMap<String, usize> {
        self.schema
            .columns
            .iter()
            .enumerate()
            .map(|(i, col)| (col.name.clone(), i))
            .collect()
    }

    /// Return a clone of the slot layout Arc.
    pub fn get_layout(&self) -> Arc<SlotLayout> {
        Arc::clone(&self.layout)
    }

    /// Get value by slot ID (fast path using layout).
    /// Returns None if row index out of bounds or no such slot.
    pub fn get_by_slot(&self, row_idx: usize, slot: SlotId) -> Option<Value> {
        self.rows
            .get(row_idx)
            .and_then(|row| row.get(slot).cloned())
    }

    /// Extract an entire column as a Vec<Value>.
    ///
    /// This is the primary API for selective columnarization: callers that
    /// need to repeatedly access the same column (e.g. sorting, hashing,
    /// aggregation) should pull it once via `get_column()` and iterate the
    /// returned `Vec<Value>` rather than calling `get_by_slot` per row.
    ///
    /// When no columnar representation exists yet, it is materialised and
    /// cached here so repeated pulls do not re-transpose the rows (lazy
    /// single-copy storage — sources no longer proactively materialise).
    ///
    /// When a typed column layout exists, the value cache is materialised
    /// from the typed columns (raw-to-`Value` conversion) instead of per-row
    /// cloning; the returned semantics are identical.
    ///
    /// Returns `None` if `slot` is out of range for the layout.
    pub fn get_column(&mut self, slot: SlotId) -> Option<Vec<Value>> {
        if slot >= self.layout.len() {
            return None;
        }
        if self.columns.is_none() && !self.rows.is_empty() {
            self.materialize_columns();
        }
        if let Some(ref columns) = self.columns {
            return columns.get(slot).cloned();
        }
        Some(self.rows.iter().map(|row| row[slot].clone()).collect())
    }

    /// Borrow an entire column as `Vec<&Value>` — no per-value cloning.
    ///
    /// Useful when the caller only needs to inspect column values (e.g.
    /// hash‑key extraction, sort‑key comparison) without taking ownership.
    pub fn column_ref(&self, slot: SlotId) -> Option<Vec<&Value>> {
        if slot >= self.layout.len() {
            return None;
        }
        Some(self.rows.iter().map(|row| &row[slot]).collect())
    }

    /// Value at `(row_idx, slot)` served from the typed layout when present
    /// (O(1) for typed columns). Falls back to the row-major path.
    pub fn get_typed_by_slot(&self, row_idx: usize, slot: SlotId) -> Option<Value> {
        if let Some(ref typed) = self.typed_columns {
            if let Some(col) = typed.get(slot) {
                return col.value_at(row_idx);
            }
        }
        self.get_by_slot(row_idx, slot)
    }

    // ── Typed column layout ──

    /// Build the typed column layout from the current rows.
    ///
    /// One [`TypedColumn`] per slot: pure `BigInt` columns become `I64`,
    /// pure `Double` columns `F64`, pure `Int` columns `I32`; columns with
    /// NULLs, mixed types, or non-scalar values fall back to `Fallback`.
    ///
    /// Returns the estimated additional heap bytes (for memory accounting).
    /// No-op when the typed layout is disabled (rollback switch) or already
    /// built, or when the chunk is empty.
    pub fn build_typed_columns(&mut self) -> usize {
        if !typed_columns_enabled() || self.typed_columns.is_some() {
            return 0;
        }
        let num_cols = self.num_columns();
        if self.rows.is_empty() || num_cols == 0 {
            return 0;
        }
        let num_rows = self.rows.len();
        let mut typed = Vec::with_capacity(num_cols);
        let mut extra_bytes = 0usize;
        for col_idx in 0..num_cols {
            let first = &self.rows[0][col_idx];
            let kind = match first {
                Value::BigInt(_) => Some(TypedKind::I64),
                Value::Double(_) => Some(TypedKind::F64),
                Value::Int(_) => Some(TypedKind::I32),
                _ => None,
            };
            let Some(kind) = kind else {
                typed.push(TypedColumn::Fallback(
                    self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                ));
                continue;
            };
            let mut ok = true;
            let column = match kind {
                TypedKind::I64 => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for row in &self.rows {
                        match row[col_idx] {
                            Value::BigInt(v) => buf.push(v),
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf.capacity() * std::mem::size_of::<i64>();
                        TypedColumn::I64(buf)
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
                TypedKind::F64 => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for row in &self.rows {
                        match row[col_idx] {
                            Value::Double(v) => buf.push(v),
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf.capacity() * std::mem::size_of::<f64>();
                        TypedColumn::F64(buf)
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
                TypedKind::I32 => {
                    let mut buf = Vec::with_capacity(num_rows);
                    for row in &self.rows {
                        match row[col_idx] {
                            Value::Int(v) => buf.push(v),
                            _ => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        extra_bytes += buf.capacity() * std::mem::size_of::<i32>();
                        TypedColumn::I32(buf)
                    } else {
                        TypedColumn::Fallback(
                            self.rows.iter().map(|row| row[col_idx].clone()).collect(),
                        )
                    }
                }
            };
            typed.push(column);
        }
        self.typed_columns = Some(typed);
        extra_bytes
    }

    /// Borrow the typed column at `slot`, if present.
    pub fn typed_column(&self, slot: SlotId) -> Option<&TypedColumn> {
        self.typed_columns.as_ref().and_then(|cols| cols.get(slot))
    }

    // ── Selection vectors ──

    /// Attach a selection vector to this chunk.
    ///
    /// `indices` must be sorted, unique, and within `rows.len()`; only these
    /// rows are visible to downstream consumers.
    pub fn with_selection(mut self, indices: Vec<usize>) -> Self {
        debug_assert!(indices.is_sorted() && indices.windows(2).all(|w| w[0] < w[1]));
        debug_assert!(indices.last().is_none_or(|&i| i < self.rows.len()));
        self.selection = Some(indices);
        self
    }

    /// The attached selection vector, or `None` when all rows are visible.
    pub fn selection(&self) -> Option<&[usize]> {
        self.selection.as_deref()
    }

    /// Number of visible rows.
    pub fn visible_count(&self) -> usize {
        self.selection
            .as_ref()
            .map(Vec::len)
            .unwrap_or(self.rows.len())
    }

    /// Indices of all visible rows.
    ///
    /// When no selection is attached, returns `(0..rows.len())` as a fresh
    /// Vec. Prefer iterating the returned slice when `selection()` is `Some`.
    pub fn visible_indices(&self) -> Vec<usize> {
        match &self.selection {
            Some(indices) => indices.clone(),
            None => (0..self.rows.len()).collect(),
        }
    }

    /// Whether a row at `idx` is visible (O(1) when no selection).
    pub fn is_visible(&self, idx: usize) -> bool {
        match &self.selection {
            None => idx < self.rows.len(),
            Some(indices) => indices.binary_search(&idx).is_ok(),
        }
    }

    /// Take the attached selection vector, leaving `None`.
    pub fn take_selection(&mut self) -> Option<Vec<usize>> {
        self.selection.take()
    }

    /// Materialize the selection vector in place.
    ///
    /// Gathers the visible rows (and the typed/columnar caches) into the
    /// front of the chunk and clears the selection. Returns `true` when a
    /// selection was actually materialized, `false` when nothing changed
    /// (chunk was already fully materialized).
    ///
    /// Selection-aware operators (Filter/Project/Join probe/Aggregate) call
    /// this implicitly; all other operators must call it after `advance()`
    /// before consuming `rows`, so a selection never leaks past a boundary.
    pub fn materialize_selection(&mut self) -> bool {
        let Some(indices) = self.selection.take() else {
            return false;
        };
        let mut selected = Vec::with_capacity(indices.len());
        for &i in &indices {
            selected.push(std::mem::take(&mut self.rows[i]));
        }
        self.rows = selected;
        self.columns = self.columns.as_ref().map(|cols| {
            cols.iter()
                .map(|col| indices.iter().map(|&i| col[i].clone()).collect())
                .collect()
        });
        self.typed_columns = self.typed_columns.as_ref().map(|cols| {
            cols.iter()
                .map(|col| gather_typed_column(col, &indices))
                .collect()
        });
        if let Some(stats) = &self.columnar_stats {
            stats.record_selection_materialized();
        }
        true
    }

    /// Row `i` as a slice (rows are always fully stored; selection is
    /// orthogonal — use `visible_indices()` to enumerate visible rows).
    pub fn row_at(&self, i: usize) -> &[Value] {
        &self.rows[i]
    }

    /// Create a new chunk containing only rows at the given `indices`.
    ///
    /// The layout and schema are shared (cheap `Arc::clone`); rows are
    /// moved out of `self` one by one.  This is more efficient than
    /// filtering row‑by‑row in a loop because the schema/layout copy is
    /// amortised over all selected rows.
    ///
    /// The typed column layout is gathered by `indices` so downstream
    /// consumers keep the typed fast path.
    ///
    /// Panics if any index is out of bounds.
    pub fn take_indices(&mut self, indices: &[usize]) -> Self {
        let layout = Arc::clone(&self.layout);
        let schema = Arc::clone(&self.schema);
        let mut selected = Vec::with_capacity(indices.len());
        for &i in indices {
            selected.push(std::mem::take(&mut self.rows[i]));
        }
        let typed_columns = self.typed_columns.as_ref().map(|cols| {
            cols.iter()
                .map(|col| gather_typed_column(col, indices))
                .collect()
        });
        // The Value columnar cache rebuild is deferred — invalidate it here
        // and let the next `get_column` materialise lazily from the selected
        // rows (or from the gathered typed layout).
        Self {
            rows: selected,
            columns: None,
            typed_columns,
            selection: None,
            schema,
            layout,
            memory_reservation: self.memory_reservation.take(),
            columnar_stats: self.columnar_stats.clone(),
        }
    }

    /// Return a selection vector: indices of rows that satisfy `pred`.
    ///
    /// Avoids cloning rows during predicate evaluation; the caller can
    /// pass the resulting indices to `take_indices` or `select`.
    pub fn filter_indices<F>(&self, layout: Arc<SlotLayout>, mut pred: F) -> Vec<usize>
    where
        F: FnMut(&[Value], &SlotLayout) -> bool,
    {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| if pred(row, &layout) { Some(i) } else { None })
            .collect()
    }

    // ── M4: view, slice ──

    /// Create a zero-copy view into this chunk's rows.
    pub fn view(&self) -> ChunkView<'_> {
        ChunkView { rows: &self.rows }
    }

    /// Move a range of rows [start, end) into a new chunk.
    ///
    /// Uses `std::mem::take` to avoid cloning.  The source rows at
    /// those indices are replaced with empty `Vec`s.
    ///
    /// The typed column layout is gathered to match.
    ///
    /// Panics if `end > self.rows.len()`.
    pub fn slice(&mut self, start: usize, end: usize) -> Self {
        assert!(end <= self.rows.len(), "slice end out of bounds");
        let layout = Arc::clone(&self.layout);
        let schema = Arc::clone(&self.schema);
        let mut selected = Vec::with_capacity(end - start);
        for i in start..end {
            selected.push(std::mem::take(&mut self.rows[i]));
        }
        let indices: Vec<usize> = (start..end).collect();
        let typed_columns = self.typed_columns.as_ref().map(|cols| {
            cols.iter()
                .map(|col| gather_typed_column(col, &indices))
                .collect()
        });
        // Defer the Value columnar cache rebuild, mirroring `take_indices`.
        Self {
            rows: selected,
            columns: None,
            typed_columns,
            selection: None,
            schema,
            layout,
            memory_reservation: self.memory_reservation.take(),
            columnar_stats: self.columnar_stats.clone(),
        }
    }

    /// Convert row-major data to column-major in place.
    ///
    /// After calling this, `self.columns` is populated and can be accessed
    /// without per-row cloning via `get_column()`.
    ///
    /// When a typed column layout is present, the value cache is derived
    /// from it (raw-to-`Value` conversion) instead of per-row cloning.
    pub fn materialize_columns(&mut self) {
        if self.columns.is_some() {
            return;
        }
        if let Some(ref typed) = self.typed_columns {
            if typed.len() == self.num_columns() && !self.rows.is_empty() {
                self.columns = Some(typed.iter().map(TypedColumn::to_values).collect());
                return;
            }
        }
        let num_cols = self.num_columns();
        if self.rows.is_empty() || num_cols == 0 {
            self.columns = Some(Vec::new());
            return;
        }
        let num_rows = self.rows.len();
        let mut columns = Vec::with_capacity(num_cols);
        for col_idx in 0..num_cols {
            let mut col = Vec::with_capacity(num_rows);
            for row in &self.rows {
                col.push(row[col_idx].clone());
            }
            columns.push(col);
        }
        self.columns = Some(columns);
    }

    /// Get the columnar representation, materializing it if needed.
    pub fn get_or_materialize_columns(&mut self) -> &[Vec<Value>] {
        self.materialize_columns();
        self.columns.as_ref().unwrap()
    }

    // ── Batch expression evaluation ──

    /// Evaluate multiple expressions in a single pass, returning one column per expression.
    ///
    /// When all expressions are simple (Variable, Literal), this avoids redundant
    /// column materialization. Falls back to per-expression evaluation for complex cases.
    pub fn evaluate_expressions(
        &mut self,
        expressions: &[Expression],
        params: Option<&Arc<HashMap<String, Value>>>,
    ) -> Result<Vec<Vec<Value>>, ExpressionError> {
        if self.rows.is_empty() {
            return Ok(vec![Vec::new(); expressions.len()]);
        }
        // Fast path: all expressions are Variables — extract columns directly.
        // get_column() transposes from rows on demand, no full materialization needed.
        if expressions
            .iter()
            .all(|e| matches!(e, Expression::Variable(_)))
        {
            let mut columns = Vec::with_capacity(expressions.len());
            for expr in expressions {
                if let Expression::Variable(name) = expr {
                    let slot = self
                        .layout
                        .slot_id(name)
                        .ok_or_else(|| ExpressionError::undefined_variable(name))?;
                    let col = self
                        .get_column(slot)
                        .ok_or_else(|| ExpressionError::undefined_variable(name))?;
                    columns.push(col);
                }
            }
            for _ in 0..expressions.len() {
                self.count_columnar(true);
            }
            return Ok(columns);
        }
        // Fast path: all are Literal expressions
        if expressions
            .iter()
            .all(|e| matches!(e, Expression::Literal(_)))
        {
            let mut columns = Vec::with_capacity(expressions.len());
            for expr in expressions {
                if let Expression::Literal(v) = expr {
                    columns.push(vec![v.clone(); self.rows.len()]);
                }
            }
            for _ in 0..expressions.len() {
                self.count_columnar(true);
            }
            return Ok(columns);
        }
        // Fall back to individual columnar evaluation
        let mut results = Vec::with_capacity(expressions.len());
        for expr in expressions {
            results.push(self.evaluate_expression(expr, params)?);
        }
        Ok(results)
    }

    /// Evaluate an expression against every row in this chunk, returning one result per row.
    ///
    /// Uses a columnar batch path for simple expressions (Literal, Variable, Unary,
    /// Binary, TypeCast, and Property-on-Variable), falling back to per-row evaluation
    /// for complex expressions (Function, Aggregate, Case, Subquery, etc.).
    ///
    /// When the chunk carries a typed column layout, Binary/Unary/TypeCast
    /// evaluation on typed columns runs directly on the raw `Vec<i64>` /
    /// `Vec<f64>` / `Vec<i32>` buffers (batch, auto-vectorizable) and converts
    /// the result to `Vec<Value>` once at the end. Results are always computed
    /// for ALL rows — a chunk-level selection vector does not restrict
    /// evaluation.
    ///
    /// `params` provides parameter values (for `$name` resolution), shared across all rows.
    pub fn evaluate_expression(
        &mut self,
        expression: &Expression,
        params: Option<&Arc<HashMap<String, Value>>>,
    ) -> Result<Vec<Value>, ExpressionError> {
        if self.rows.is_empty() {
            return Ok(Vec::new());
        }
        // Fast columnar batch path
        if let Ok((result, typed_hit)) = self.try_evaluate_columnar(expression, params) {
            self.count_columnar(true);
            if typed_hit {
                self.count_typed_hit();
            }
            return Ok(result);
        }
        self.count_columnar(false);
        debug_assert!(
            !self.columnar_promise_holds(expression),
            "flat column promise broken: expression {:?} should have hit the \
             columnar path but fell back to per-row evaluation",
            expression
        );
        // Fall back to per-row evaluation
        self.evaluate_expression_per_row(expression, params)
    }

    /// Evaluate `expression` producing one value per *visible* row.
    ///
    /// When the chunk carries a selection vector, only the selected rows are
    /// evaluated (O(visible)); when no selection is attached this delegates
    /// to [`evaluate_expression`](Self::evaluate_expression) so behaviour is
    /// identical to the pre-selection code path.
    ///
    /// `params` provides parameter values (for `$name` resolution), shared across all rows.
    pub fn evaluate_expression_visible(
        &mut self,
        expression: &Expression,
        params: Option<&Arc<HashMap<String, Value>>>,
    ) -> Result<Vec<Value>, ExpressionError> {
        let Some(sel) = self.selection().map(|s| s.to_vec()) else {
            return self.evaluate_expression(expression, params);
        };
        // Fast path: Variable / flat Property on a (typed) column — O(visible)
        // indexed materialization instead of a full evaluation pass.
        let slot: Option<SlotId> = match expression {
            Expression::Variable(name) => self.layout.slot_id(name),
            Expression::Property { object, property }
                if matches!(object.as_ref(), Expression::Variable(_)) =>
            {
                if let Expression::Variable(var) = object.as_ref() {
                    self.layout.slot_id(&format!("{}.{}", var, property))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(slot) = slot {
            let mut out = Vec::with_capacity(sel.len());
            for &i in &sel {
                match self.get_typed_by_slot(i, slot) {
                    Some(v) => out.push(v),
                    None => return Err(ExpressionError::undefined_variable("column slot")),
                }
            }
            self.count_columnar(true);
            return Ok(out);
        }
        // General path: per visible row via a row context.
        let layout = self.get_layout();
        let mut out = Vec::with_capacity(sel.len());
        for &i in &sel {
            let row = &self.rows[i];
            let mut ctx = match params {
                Some(p) => BorrowedRowContext::with_parameters(row, layout.clone(), p.clone()),
                None => BorrowedRowContext::new(row, layout.clone()),
            };
            out.push(ExpressionEvaluator::evaluate(expression, &mut ctx)?);
        }
        self.count_columnar(false);
        Ok(out)
    }

    /// Columnar batch evaluation path — returns Err if the expression is too complex.
    ///
    /// The `bool` in the Ok value reports whether the typed batch fast path
    /// served the result.
    fn try_evaluate_columnar(
        &mut self,
        expression: &Expression,
        params: Option<&Arc<HashMap<String, Value>>>,
    ) -> Result<(Vec<Value>, bool), ExpressionError> {
        // Build a column cache to avoid redundant get_column calls
        // when the same variable appears in multiple sub-expressions.
        let mut col_cache: HashMap<String, Vec<Value>> = HashMap::new();
        self.collect_variables(expression, &mut col_cache);
        let mut typed_hit = false;
        let result = self.eval_with_cache(expression, &col_cache, params, &mut typed_hit)?;
        Ok((result, typed_hit))
    }

    /// Collect all Variable references from an expression tree into col_cache.
    ///
    /// Only direct `Expression::Variable` references are cached: property
    /// expressions resolve via compound slots (`{var}.{prop}`) in
    /// `eval_with_cache`, so caching the whole entity column per property
    /// access would only add per-row deep copies that are never read.
    fn collect_variables(
        &mut self,
        expr: &Expression,
        col_cache: &mut HashMap<String, Vec<Value>>,
    ) {
        match expr {
            Expression::Variable(name) => {
                if !col_cache.contains_key(name) {
                    if let Some(slot) = self.layout.slot_id(name) {
                        if let Some(col) = self.get_column(slot) {
                            col_cache.insert(name.clone(), col);
                        }
                    }
                }
            }
            Expression::Binary { left, right, .. } => {
                self.collect_variables(left, col_cache);
                self.collect_variables(right, col_cache);
            }
            Expression::Unary { operand, .. } => {
                self.collect_variables(operand, col_cache);
            }
            Expression::TypeCast { expression, .. } => {
                self.collect_variables(expression, col_cache);
            }
            _ => {}
        }
    }

    /// Evaluate expression using a pre-populated column cache.
    ///
    /// First tries the typed batch fast path; when it applies
    /// (`typed_used` is set), the whole expression tree was evaluated on raw
    /// typed buffers and the result converted to `Vec<Value>` once.
    fn eval_with_cache(
        &mut self,
        expression: &Expression,
        col_cache: &HashMap<String, Vec<Value>>,
        params: Option<&Arc<HashMap<String, Value>>>,
        typed_used: &mut bool,
    ) -> Result<Vec<Value>, ExpressionError> {
        // Typed batch fast path: all leaves are typed columns/literals.
        if let Some(batch) = self.try_eval_typed_batch(expression, params)? {
            *typed_used = true;
            return Ok(batch.into_values());
        }
        match expression {
            Expression::Literal(v) => Ok(vec![v.clone(); self.rows.len()]),

            Expression::Variable(name) => {
                // Use column cache if available, otherwise fall back to get_column
                if let Some(col) = col_cache.get(name) {
                    return Ok(col.clone());
                }
                let slot = self
                    .layout
                    .slot_id(name)
                    .ok_or_else(|| ExpressionError::undefined_variable(name))?;
                self.get_column(slot)
                    .ok_or_else(|| ExpressionError::undefined_variable(name))
            }

            Expression::Parameter(name) => {
                let val = params
                    .and_then(|p| p.get(name).cloned())
                    .ok_or_else(|| ExpressionError::undefined_parameter(name))?;
                Ok(vec![val; self.rows.len()])
            }

            Expression::Unary { op, operand } => {
                let values = self.eval_with_cache(operand, col_cache, params, typed_used)?;
                values
                    .into_iter()
                    .map(|v| UnaryOperationEvaluator::evaluate(op, &v))
                    .collect()
            }

            Expression::Binary { left, op, right } => {
                let left_values = self.eval_with_cache(left, col_cache, params, typed_used)?;
                let right_values = self.eval_with_cache(right, col_cache, params, typed_used)?;
                left_values
                    .into_iter()
                    .zip(right_values)
                    .map(|(l, r)| BinaryOperationEvaluator::evaluate(&l, op, &r))
                    .collect()
            }

            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let values = self.eval_with_cache(expression, col_cache, params, typed_used)?;
                values
                    .into_iter()
                    .map(|v| ExpressionEvaluator::eval_type_cast(&v, target_type))
                    .collect()
            }

            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    let compound = format!("{}.{}", var_name, property);
                    // Fast path: compound name exists as a direct column
                    if let Some(slot) = self.layout.slot_id(&compound) {
                        if let Some(col) = self.get_column(slot) {
                            return Ok(col);
                        }
                    }
                    // Medium path: object is a Variable but property is not a column —
                    // we need per-row property extraction, fall back.
                    return Err(ExpressionError::type_error(
                        "Property access requires per-row evaluation",
                    ));
                }
                // Complex object expression — fall back
                Err(ExpressionError::type_error(
                    "Property access requires per-row evaluation",
                ))
            }

            _ => Err(ExpressionError::type_error(
                "Expression requires per-row evaluation",
            )),
        }
    }

    /// Per-row fallback for complex expressions.
    fn evaluate_expression_per_row(
        &self,
        expression: &Expression,
        params: Option<&Arc<HashMap<String, Value>>>,
    ) -> Result<Vec<Value>, ExpressionError> {
        let layout = self.get_layout();
        let mut results = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let mut ctx = match params {
                Some(p) => BorrowedRowContext::with_parameters(row, layout.clone(), p.clone()),
                None => BorrowedRowContext::new(row, layout.clone()),
            };
            results.push(ExpressionEvaluator::evaluate(expression, &mut ctx)?);
        }
        Ok(results)
    }

    /// Record a columnar fast-path hit/miss on the attached counters.
    fn count_columnar(&self, hit: bool) {
        if let Some(stats) = &self.columnar_stats {
            if hit {
                stats.record_hit();
            } else {
                stats.record_miss();
            }
        }
    }

    /// Record a typed batch fast-path hit on the attached counters.
    fn count_typed_hit(&self) {
        if let Some(stats) = &self.columnar_stats {
            stats.record_typed_hit();
        }
    }

    // ── Typed batch evaluation ──

    /// Evaluate `expression` on typed raw buffers, when every leaf resolves to
    /// a typed column or a matching typed literal.
    ///
    /// Returns `Ok(None)` when the expression tree cannot be evaluated in
    /// typed space (mixed kinds, non-scalar nodes, NULL columns) — the caller
    /// then falls back to the value-based path with identical semantics.
    fn try_eval_typed_batch(
        &mut self,
        expression: &Expression,
        params: Option<&Arc<HashMap<String, Value>>>,
    ) -> Result<Option<TypedBatch>, ExpressionError> {
        match expression {
            Expression::Literal(v) => Ok(typed_literal_batch(v, self.rows.len())),
            Expression::Parameter(name) => {
                let val = params
                    .and_then(|p| p.get(name).cloned())
                    .ok_or_else(|| ExpressionError::undefined_parameter(name))?;
                Ok(typed_literal_batch(&val, self.rows.len()))
            }
            Expression::Variable(name) => {
                let slot = match self.layout.slot_id(name) {
                    Some(slot) => slot,
                    None => return Ok(None),
                };
                Ok(self.typed_column(slot).and_then(typed_column_batch))
            }
            Expression::Unary { op, operand } => {
                let Some(batch) = self.try_eval_typed_batch(operand, params)? else {
                    return Ok(None);
                };
                Ok(typed_unary_batch(op, batch))
            }
            Expression::Binary { left, op, right } => {
                let Some(left_batch) = self.try_eval_typed_batch(left, params)? else {
                    return Ok(None);
                };
                let Some(right_batch) = self.try_eval_typed_batch(right, params)? else {
                    return Ok(None);
                };
                typed_binary_batch(op, &left_batch, &right_batch)
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let Some(batch) = self.try_eval_typed_batch(expression, params)? else {
                    return Ok(None);
                };
                Ok(typed_cast_batch(batch, target_type))
            }
            _ => Ok(None),
        }
    }

    /// Whether the whole expression tree is promised to hit the columnar path.
    ///
    /// True only when every node is supported by the columnar evaluator and
    /// every `{var}.{prop}` property access has a matching compound slot in
    /// this chunk's layout. Debug-only: a miss for such an expression means
    /// the flat-column promise was broken and would silently regress
    /// performance.
    #[cfg(debug_assertions)]
    fn columnar_promise_holds(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Literal(_) | Expression::Parameter(_) | Expression::Variable(_) => true,
            Expression::Unary { operand, .. } => self.columnar_promise_holds(operand),
            Expression::Binary { left, right, .. } => {
                self.columnar_promise_holds(left) && self.columnar_promise_holds(right)
            }
            Expression::TypeCast { expression, .. } => self.columnar_promise_holds(expression),
            Expression::Property { object, property } => {
                if let Expression::Variable(var) = object.as_ref() {
                    let compound = format!("{}.{}", var, property);
                    return self.layout.slot_id(&compound).is_some();
                }
                false
            }
            _ => false,
        }
    }

    #[cfg(not(debug_assertions))]
    fn columnar_promise_holds(&self, _expr: &Expression) -> bool {
        false
    }
}

/// A batch of raw typed values produced by the typed evaluator.
///
/// Mirrors `Value::BigInt`/`Value::Double`/`Value::Int`/`Value::Bool` in
/// raw space; converted to `Vec<Value>` once at the end of evaluation.
#[derive(Debug, Clone)]
enum TypedBatch {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
}

impl TypedBatch {
    fn into_values(self) -> Vec<Value> {
        match self {
            TypedBatch::I64(v) => v.into_iter().map(Value::BigInt).collect(),
            TypedBatch::F64(v) => v.into_iter().map(Value::Double).collect(),
            TypedBatch::I32(v) => v.into_iter().map(Value::Int).collect(),
            TypedBatch::Bool(v) => v.into_iter().map(Value::Bool).collect(),
        }
    }
}

/// Borrow a typed column as a raw batch (`Fallback` columns are not typed).
fn typed_column_batch(column: &TypedColumn) -> Option<TypedBatch> {
    match column {
        TypedColumn::I64(v) => Some(TypedBatch::I64(v.clone())),
        TypedColumn::F64(v) => Some(TypedBatch::F64(v.clone())),
        TypedColumn::I32(v) => Some(TypedBatch::I32(v.clone())),
        TypedColumn::Fallback(_) => None,
    }
}

/// Replicate a literal into a raw batch of `n` rows, when the literal has a
/// typed scalar kind (BigInt/Double/Int/Bool).
fn typed_literal_batch(value: &Value, n: usize) -> Option<TypedBatch> {
    match value {
        Value::BigInt(v) => Some(TypedBatch::I64(vec![*v; n])),
        Value::Double(v) => Some(TypedBatch::F64(vec![*v; n])),
        Value::Int(v) => Some(TypedBatch::I32(vec![*v; n])),
        Value::Bool(v) => Some(TypedBatch::Bool(vec![*v; n])),
        _ => None,
    }
}

/// Unary operators on raw typed batches.
///
/// Mirrors `UnaryOperationEvaluator` for the supported subset; anything else
/// returns `None` so the caller falls back to the value path.
fn typed_unary_batch(op: &UnaryOperator, batch: TypedBatch) -> Option<TypedBatch> {
    match op {
        UnaryOperator::Plus => Some(batch),
        UnaryOperator::Minus => match batch {
            TypedBatch::I64(v) => Some(TypedBatch::I64(
                v.into_iter().map(i64::wrapping_neg).collect(),
            )),
            TypedBatch::F64(v) => Some(TypedBatch::F64(v.into_iter().map(|x| -x).collect())),
            TypedBatch::I32(v) => Some(TypedBatch::I32(
                v.into_iter().map(i32::wrapping_neg).collect(),
            )),
            TypedBatch::Bool(_) => None,
        },
        UnaryOperator::Not => match batch {
            TypedBatch::Bool(v) => Some(TypedBatch::Bool(v.into_iter().map(|b| !b).collect())),
            _ => None,
        },
        _ => None,
    }
}

/// Binary operators on raw typed batches.
///
/// Mirrors `BinaryOperationEvaluator` / `Value` comparison and arithmetic
/// semantics for the supported subset (same-kind operands only); mixed kinds
/// and unsupported operators return `None` so the caller falls back to the
/// value path, which handles cross-type coercion exactly.
fn typed_binary_batch(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Result<Option<TypedBatch>, ExpressionError> {
    use BinaryOperator::*;
    match op {
        Equal | NotEqual | LessThan | LessThanOrEqual | GreaterThan | GreaterThanOrEqual => {
            Ok(compare_typed_batches(op, left, right))
        }
        Add | Subtract | Multiply => Ok(arith_typed_batches(op, left, right)),
        And | Or => match (left, right) {
            (TypedBatch::Bool(l), TypedBatch::Bool(r)) => {
                let vals = l
                    .iter()
                    .zip(r)
                    .map(|(&a, &b)| match op {
                        And => a & b,
                        Or => a | b,
                        _ => unreachable!("matched And/Or above"),
                    })
                    .collect();
                Ok(Some(TypedBatch::Bool(vals)))
            }
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

/// Comparison operators on same-kind raw batches.
fn compare_typed_batches(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    let matches = |ordering: Ordering| -> bool {
        match op {
            BinaryOperator::Equal => ordering == Ordering::Equal,
            BinaryOperator::NotEqual => ordering != Ordering::Equal,
            BinaryOperator::LessThan => ordering == Ordering::Less,
            BinaryOperator::LessThanOrEqual => ordering != Ordering::Greater,
            BinaryOperator::GreaterThan => ordering == Ordering::Greater,
            BinaryOperator::GreaterThanOrEqual => ordering != Ordering::Less,
            _ => unreachable!("called with a non-comparison operator"),
        }
    };
    // Ordering comparison mirrors Value::cmp for same-kind pairs.
    let batch = match (left, right) {
        (TypedBatch::I64(l), TypedBatch::I64(r)) => {
            TypedBatch::Bool(l.iter().zip(r).map(|(&a, &b)| matches(a.cmp(&b))).collect())
        }
        (TypedBatch::F64(l), TypedBatch::F64(r)) => TypedBatch::Bool(
            l.iter()
                .zip(r)
                .map(|(&a, &b)| matches(cmp_f64_value(a, b)))
                .collect(),
        ),
        (TypedBatch::I32(l), TypedBatch::I32(r)) => {
            TypedBatch::Bool(l.iter().zip(r).map(|(&a, &b)| matches(a.cmp(&b))).collect())
        }
        (TypedBatch::Bool(l), TypedBatch::Bool(r))
            if matches!(op, BinaryOperator::Equal | BinaryOperator::NotEqual) =>
        {
            TypedBatch::Bool(l.iter().zip(r).map(|(&a, &b)| matches(a.cmp(&b))).collect())
        }
        _ => return None,
    };
    Some(batch)
}

/// f64 ordering mirroring `Value::cmp_f64` (NaN ordering: NaN == NaN, NaN < x).
fn cmp_f64_value(a: f64, b: f64) -> Ordering {
    if a.is_nan() && b.is_nan() {
        Ordering::Equal
    } else if a.is_nan() {
        Ordering::Less
    } else if b.is_nan() {
        Ordering::Greater
    } else {
        a.partial_cmp(&b).unwrap_or(Ordering::Equal)
    }
}

/// Arithmetic operators on same-kind raw batches (wrapping for ints).
fn arith_typed_batches(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    use BinaryOperator::{Add, Multiply, Subtract};
    match (left, right) {
        (TypedBatch::I64(l), TypedBatch::I64(r)) => Some(TypedBatch::I64(
            l.iter()
                .zip(r)
                .map(|(&a, &b)| match op {
                    Add => a.wrapping_add(b),
                    Subtract => a.wrapping_sub(b),
                    Multiply => a.wrapping_mul(b),
                    _ => unreachable!("arith only"),
                })
                .collect(),
        )),
        (TypedBatch::F64(l), TypedBatch::F64(r)) => Some(TypedBatch::F64(
            l.iter()
                .zip(r)
                .map(|(&a, &b)| match op {
                    Add => a + b,
                    Subtract => a - b,
                    Multiply => a * b,
                    _ => unreachable!("arith only"),
                })
                .collect(),
        )),
        (TypedBatch::I32(l), TypedBatch::I32(r)) => Some(TypedBatch::I32(
            l.iter()
                .zip(r)
                .map(|(&a, &b)| match op {
                    Add => a.wrapping_add(b),
                    Subtract => a.wrapping_sub(b),
                    Multiply => a.wrapping_mul(b),
                    _ => unreachable!("arith only"),
                })
                .collect(),
        )),
        _ => None,
    }
}

/// Type casts on raw typed batches.
///
/// Mirrors `ExpressionEvaluator::eval_type_cast` for numeric targets. Casts
/// that may produce NULL (e.g. non-finite f64 → int) are NOT served by the
/// typed path and fall back to the value path.
fn typed_cast_batch(
    batch: TypedBatch,
    target_type: &crate::core::types::DataType,
) -> Option<TypedBatch> {
    use crate::core::types::DataType;
    match target_type {
        // DataType::Int => value.to_int() (returns BigInt — replicated verbatim).
        DataType::Int | DataType::BigInt => match batch {
            TypedBatch::I64(v) => Some(TypedBatch::I64(v)),
            TypedBatch::I32(v) => Some(TypedBatch::I64(v.into_iter().map(i64::from).collect())),
            _ => None,
        },
        DataType::Double => match batch {
            TypedBatch::F64(v) => Some(TypedBatch::F64(v)),
            TypedBatch::I64(v) => Some(TypedBatch::F64(v.into_iter().map(|x| x as f64).collect())),
            TypedBatch::I32(v) => Some(TypedBatch::F64(v.into_iter().map(|x| x as f64).collect())),
            _ => None,
        },
        DataType::Bool => match batch {
            TypedBatch::I64(v) => Some(TypedBatch::Bool(v.into_iter().map(|x| x != 0).collect())),
            TypedBatch::F64(v) => Some(TypedBatch::Bool(v.into_iter().map(|x| x != 0.0).collect())),
            TypedBatch::I32(v) => Some(TypedBatch::Bool(v.into_iter().map(|x| x != 0).collect())),
            TypedBatch::Bool(v) => Some(TypedBatch::Bool(v)),
        },
        _ => None,
    }
}

/// Gather a typed column's entries at `indices`.
fn gather_typed_column(column: &TypedColumn, indices: &[usize]) -> TypedColumn {
    match column {
        TypedColumn::I64(v) => TypedColumn::I64(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::F64(v) => TypedColumn::F64(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::I32(v) => TypedColumn::I32(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::Fallback(v) => {
            TypedColumn::Fallback(indices.iter().map(|&i| v[i].clone()).collect())
        }
    }
}

/// A zero-copy view into a slice of rows within a [`DataChunk`].
///
/// Created by [`DataChunk::view`].  The view borrows the parent chunk
/// and does not own its data.
#[derive(Debug)]
pub struct ChunkView<'a> {
    pub(crate) rows: &'a [Vec<crate::core::Value>],
}

impl ChunkView<'_> {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn row(&self, idx: usize) -> Option<&[crate::core::Value]> {
        self.rows.get(idx).map(|r| r.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::Expression;
    use crate::core::types::operators::BinaryOperator;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::Vertex;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_data_chunk_creation() {
        let rows = vec![vec![Value::string("a"), Value::string("b")]];
        let chunk = DataChunk::from_rows(rows);
        assert_eq!(chunk.len(), 1);
        assert_eq!(chunk.num_columns(), 2);
    }

    #[test]
    fn test_data_chunk_empty() {
        let chunk = DataChunk::from_rows(vec![]);
        assert!(chunk.is_empty());
        assert_eq!(chunk.num_columns(), 0);
    }

    #[test]
    fn test_data_chunk_type_inference() {
        let rows = vec![vec![Value::BigInt(42), Value::string("hello")]];
        let chunk = DataChunk::from_rows(rows);
        assert_eq!(chunk.schema.columns[0].data_type, "bigint");
        assert_eq!(chunk.schema.columns[1].data_type, "string");
    }

    /// The flat-property column (`p.age`) must be served by the columnar
    /// `Property` branch.  The vertex in slot 0 deliberately carries no
    /// properties, so correct results are only possible if the compound slot
    /// is used instead of per-row property extraction.
    #[test]
    fn flat_property_column_hits_columnar_path() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
        ]));
        let rows = vec![
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
                Value::BigInt(30),
            ],
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(2)))),
                Value::BigInt(20),
            ],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        let expr = Expression::binary(
            Expression::property(Expression::variable("p"), "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(28)),
        );
        let results = chunk
            .evaluate_expression(&expr, None)
            .expect("evaluate should succeed");
        assert_eq!(results, vec![Value::Bool(true), Value::Bool(false)]);
    }

    /// The flat-column promise holds when every property access has a matching
    /// compound slot: the columnar path must hit for `p.age > 28`.
    #[cfg(debug_assertions)]
    #[test]
    fn flat_column_promise_holds_for_flat_layout() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
            "p.name".to_string(),
        ]));
        let rows = vec![vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
            Value::BigInt(30),
            Value::string("Alice"),
        ]];
        let chunk = DataChunk::new_with_layout(rows, layout);
        let expr = Expression::binary(
            Expression::property(Expression::variable("p"), "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(28)),
        );
        assert!(chunk.columnar_promise_holds(&expr));
    }

    /// The promise does not hold when a property access has no compound slot
    /// (e.g. a variable bound to a non-scan source): such expressions are
    /// legitimately evaluated per row.
    #[cfg(debug_assertions)]
    #[test]
    fn flat_column_promise_does_not_hold_without_compound_slot() {
        let layout = Arc::new(SlotLayout::from_names(&["p".to_string()]));
        let rows = vec![vec![Value::Vertex(Box::new(Vertex::with_vid(
            VertexId::from_int64(1),
        )))]];
        let chunk = DataChunk::new_with_layout(rows, layout);
        let expr = Expression::property(Expression::variable("p"), "age");
        assert!(!chunk.columnar_promise_holds(&expr));
    }

    /// Expressions with non-columnar nodes (functions) are not promised even
    /// when they contain a flat property access.
    #[cfg(debug_assertions)]
    #[test]
    fn flat_column_promise_excludes_unsupported_nodes() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
        ]));
        let rows = vec![vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
            Value::BigInt(30),
        ]];
        let chunk = DataChunk::new_with_layout(rows, layout);
        let expr = Expression::function(
            "abs".to_string(),
            vec![Expression::property(Expression::variable("p"), "age")],
        );
        assert!(!chunk.columnar_promise_holds(&expr));
    }

    /// Attached counters record one hit per successful columnar
    /// evaluation and one miss per per-row fallback.
    #[test]
    fn columnar_stats_record_hits_and_misses() {
        let stats = Arc::new(crate::query::executor::streaming::runtime::ColumnarStats::new());
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
        ]));
        let rows = vec![vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
            Value::BigInt(30),
        ]];
        let mut chunk = DataChunk::new_with_layout(rows, layout).with_columnar_stats(stats.clone());

        let simple = Expression::binary(
            Expression::property(Expression::variable("p"), "age"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(28)),
        );
        chunk
            .evaluate_expression(&simple, None)
            .expect("columnar evaluation should succeed");
        assert_eq!(stats.columnar_hits.load(Ordering::Relaxed), 1);
        assert_eq!(stats.columnar_misses.load(Ordering::Relaxed), 0);
        assert_eq!(stats.hit_rate(), 1.0);

        // A function expression is not columnar-supported: per-row fallback.
        let complex = Expression::function(
            "abs".to_string(),
            vec![Expression::property(Expression::variable("p"), "age")],
        );
        chunk
            .evaluate_expression(&complex, None)
            .expect("per-row evaluation should succeed");
        assert_eq!(stats.columnar_hits.load(Ordering::Relaxed), 1);
        assert_eq!(stats.columnar_misses.load(Ordering::Relaxed), 1);
        assert!((stats.hit_rate() - 0.5).abs() < 1e-9);
    }

    /// `get_column` lazily materialises and caches the columnar
    /// representation; `take_indices` defers the cache rebuild.
    #[test]
    fn column_cache_is_lazy_and_deferred_after_take_indices() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
        ]));
        let rows = vec![
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
                Value::BigInt(30),
            ],
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(2)))),
                Value::BigInt(20),
            ],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        assert!(chunk.columns.is_none(), "no proactive materialisation");

        let col = chunk.get_column(1).expect("age column");
        assert_eq!(col, vec![Value::BigInt(30), Value::BigInt(20)]);
        assert!(chunk.columns.is_some(), "materialised and cached on demand");

        let mut selected = chunk.take_indices(&[1]);
        assert!(
            selected.columns.is_none(),
            "columnar cache rebuild is deferred after take_indices"
        );
        assert_eq!(
            selected.get_column(1).expect("age column"),
            vec![Value::BigInt(20)]
        );
    }

    // ── Typed columns ──

    #[test]
    fn typed_columns_build_pure_bigint_column() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..100)
            .map(|i| vec![Value::BigInt((i % 1000) as i64)])
            .collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        let bytes = chunk.build_typed_columns();
        let typed = chunk.typed_column(0).expect("typed column built");
        assert!(matches!(typed, TypedColumn::I64(_)), "expected I64 layout");
        assert!(bytes > 0, "typed allocation must be accounted");
        assert_eq!(
            typed.value_at(5),
            Some(Value::BigInt(5)),
            "O(1) indexed materialization"
        );
    }

    #[test]
    fn typed_columns_fallback_on_null_and_mixed_and_string() {
        let layout = Arc::new(SlotLayout::from_names(&[
            "n".to_string(),
            "mixed".to_string(),
            "s".to_string(),
        ]));
        let rows = vec![
            vec![
                Value::Null(crate::core::value::NullType::Null),
                Value::BigInt(1),
                Value::string("a"),
            ],
            vec![Value::BigInt(2), Value::Double(2.0), Value::string("b")],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();
        assert!(matches!(
            chunk.typed_column(0),
            Some(TypedColumn::Fallback(_))
        ));
        assert!(matches!(
            chunk.typed_column(1),
            Some(TypedColumn::Fallback(_))
        ));
        assert!(matches!(
            chunk.typed_column(2),
            Some(TypedColumn::Fallback(_))
        ));
    }

    #[test]
    fn typed_columns_survive_take_indices() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();
        let mut selected = chunk.take_indices(&[0, 2, 4]);
        assert!(matches!(
            selected.typed_column(0),
            Some(TypedColumn::I64(_))
        ));
        assert_eq!(
            selected.get_column(0).expect("column"),
            vec![Value::BigInt(0), Value::BigInt(2), Value::BigInt(4)]
        );
    }

    #[test]
    fn typed_eval_matches_value_path_semantics() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..100)
            .map(|i| vec![Value::BigInt((i % 1000) as i64)])
            .collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();

        let expr = Expression::binary(
            Expression::variable("k0"),
            BinaryOperator::GreaterThan,
            Expression::literal(Value::BigInt(500)),
        );
        let typed_result = chunk.evaluate_expression(&expr, None).expect("eval");
        assert_eq!(typed_result.len(), 100);

        // Sanity: expected boolean mask.
        let expected: Vec<Value> = (0..100).map(|i| Value::Bool((i % 1000) > 500)).collect();
        assert_eq!(typed_result, expected);
    }

    #[test]
    fn typed_eval_supports_arithmetic_and_cast() {
        let layout = Arc::new(SlotLayout::from_names(&["a".to_string(), "b".to_string()]));
        let rows = vec![
            vec![Value::BigInt(40), Value::BigInt(2)],
            vec![Value::BigInt(10), Value::BigInt(5)],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();

        let add = Expression::binary(
            Expression::variable("a"),
            BinaryOperator::Add,
            Expression::variable("b"),
        );
        assert_eq!(
            chunk.evaluate_expression(&add, None).expect("add"),
            vec![Value::BigInt(42), Value::BigInt(15)]
        );

        let cast = Expression::TypeCast {
            expression: Box::new(Expression::variable("a")),
            target_type: crate::core::DataType::Double,
        };
        assert_eq!(
            chunk.evaluate_expression(&cast, None).expect("cast"),
            vec![Value::Double(40.0), Value::Double(10.0)]
        );
    }

    #[test]
    fn typed_columns_disabled_falls_back() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
        set_typed_columns_enabled(false);
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        let bytes = chunk.build_typed_columns();
        assert_eq!(bytes, 0);
        assert!(chunk.typed_column(0).is_none());
        set_typed_columns_enabled(true);
    }

    #[test]
    fn row_pool_recycles_typed_columns() {
        let pool = RowPool::new(64, 1);
        let col = pool.acquire_typed(TypedKind::I64);
        pool.release_typed(col);
        let col = pool.acquire_typed(TypedKind::I64);
        match col {
            TypedColumn::I64(v) => assert!(v.capacity() >= 64, "recycled capacity"),
            _ => panic!("expected I64 column"),
        }
    }

    // ── Selection vectors ──

    #[test]
    fn selection_attachment_and_materialization() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();

        let chunk = chunk.with_selection(vec![0, 2, 4]);
        assert_eq!(chunk.visible_count(), 3);
        assert_eq!(chunk.visible_indices(), vec![0, 2, 4]);
        assert!(chunk.is_visible(2));
        assert!(!chunk.is_visible(1));

        let mut chunk = chunk;
        assert!(chunk.materialize_selection());
        assert_eq!(chunk.visible_count(), 3);
        assert!(chunk.selection().is_none());
        // Rows were gathered in selection order and typed columns follow.
        let col = chunk.get_column(0).expect("column");
        assert_eq!(
            col,
            vec![Value::BigInt(0), Value::BigInt(2), Value::BigInt(4)]
        );
        // Second materialize is a no-op.
        assert!(!chunk.materialize_selection());
    }

    #[test]
    fn selection_preserves_typed_columns_until_materialized() {
        let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
        let rows: Vec<Vec<Value>> = (0..6).map(|i| vec![Value::BigInt(i as i64)]).collect();
        let mut chunk = DataChunk::new_with_layout(rows, layout);
        chunk.build_typed_columns();
        let mut chunk = chunk.with_selection(vec![1, 3]);
        assert!(chunk.typed_column(0).is_some(), "typed layout kept");
        chunk.materialize_selection();
        let typed = chunk.typed_column(0).expect("typed layout gathered");
        assert_eq!(typed.to_values(), vec![Value::BigInt(1), Value::BigInt(3)]);
    }
}
