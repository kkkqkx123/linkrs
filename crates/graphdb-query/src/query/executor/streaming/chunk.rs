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

use super::slot::{SlotId, SlotLayout};
use super::runtime::ColumnarStats;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryReservation;
use crate::query::executor::expression::evaluator::operations::{
    BinaryOperationEvaluator, UnaryOperationEvaluator,
};
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::expression::ExpressionError;
use crate::query::executor::streaming::context::BorrowedRowContext;
use std::collections::HashMap;
use std::sync::Arc;

const ROW_POOL_MAX_SIZE: usize = 8;

/// Pool of recycled `Vec<Vec<Value>>` allocations for DataChunk construction.
///
/// Reduces allocation overhead by reusing Vec buffers across chunk boundaries.
/// Each acquired Vec is guaranteed to have `chunk_size` capacity (not length).
pub struct RowPool {
    pool: parking_lot::Mutex<Vec<Vec<Vec<Value>>>>,
    chunk_size: usize,
    num_columns: usize,
}

impl RowPool {
    pub fn new(chunk_size: usize, num_columns: usize) -> Self {
        Self {
            pool: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
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
    /// Schema information (column names and types)
    pub schema: Arc<Schema>,
    /// Slot layout for slot-based value access.
    /// Always set for production chunks; convenience constructors auto-create
    /// from column names when no explicit layout is provided.
    pub layout: Arc<SlotLayout>,
    /// Memory reservation for this chunk's data.
    /// Dropping the chunk releases the reserved bytes.
    pub memory_reservation: Option<MemoryReservation>,
    /// Query-level columnar fast-path counters (T5 observability).
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

    /// Attach query-level columnar fast-path counters (T5 observability).
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
    /// cached here so repeated pulls do not re-transpose the rows (T4.2:
    /// lazy single-copy storage — sources no longer proactively materialise).
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

    /// Create a new chunk containing only rows at the given `indices`.
    ///
    /// The layout and schema are shared (cheap `Arc::clone`); rows are
    /// moved out of `self` one by one.  This is more efficient than
    /// filtering row‑by‑row in a loop because the schema/layout copy is
    /// amortised over all selected rows.
    ///
    /// Panics if any index is out of bounds.
    pub fn take_indices(&mut self, indices: &[usize]) -> Self {
        let layout = Arc::clone(&self.layout);
        let schema = Arc::clone(&self.schema);
        let mut selected = Vec::with_capacity(indices.len());
        for &i in indices {
            selected.push(std::mem::take(&mut self.rows[i]));
        }
        // T4.2: columnar cache rebuild is deferred — invalidate it here and
        // let the next `get_column` materialise lazily from the selected rows.
        Self {
            rows: selected,
            columns: None,
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
    /// Panics if `end > self.rows.len()`.
    pub fn slice(&mut self, start: usize, end: usize) -> Self {
        assert!(end <= self.rows.len(), "slice end out of bounds");
        let layout = Arc::clone(&self.layout);
        let schema = Arc::clone(&self.schema);
        let mut selected = Vec::with_capacity(end - start);
        for i in start..end {
            selected.push(std::mem::take(&mut self.rows[i]));
        }
        // T4.2: defer columnar cache rebuild, mirroring `take_indices`.
        Self {
            rows: selected,
            columns: None,
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
    pub fn materialize_columns(&mut self) {
        if self.columns.is_some() {
            return;
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
    ) -> Result<Vec<Vec<Value>>, ExpressionError> {        if self.rows.is_empty() {
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
        if let Ok(result) = self.try_evaluate_columnar(expression, params) {
            self.count_columnar(true);
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

    /// Columnar batch evaluation path — returns Err if the expression is too complex.
    fn try_evaluate_columnar(
        &mut self,
        expression: &Expression,
        params: Option<&Arc<HashMap<String, Value>>>,
    ) -> Result<Vec<Value>, ExpressionError> {
        // Build a column cache to avoid redundant get_column calls
        // when the same variable appears in multiple sub-expressions.
        let mut col_cache: HashMap<String, Vec<Value>> = HashMap::new();
        self.collect_variables(expression, &mut col_cache);
        self.eval_with_cache(expression, &col_cache, params)
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
    fn eval_with_cache(
        &mut self,
        expression: &Expression,
        col_cache: &HashMap<String, Vec<Value>>,
        params: Option<&Arc<HashMap<String, Value>>>,
    ) -> Result<Vec<Value>, ExpressionError> {
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
                let values = self.eval_with_cache(operand, col_cache, params)?;
                values
                    .into_iter()
                    .map(|v| UnaryOperationEvaluator::evaluate(op, &v))
                    .collect()
            }

            Expression::Binary { left, op, right } => {
                let left_values = self.eval_with_cache(left, col_cache, params)?;
                let right_values = self.eval_with_cache(right, col_cache, params)?;
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
                let values = self.eval_with_cache(expression, col_cache, params)?;
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

    /// Record a columnar fast-path hit/miss on the attached counters (T5).
    fn count_columnar(&self, hit: bool) {
        if let Some(stats) = &self.columnar_stats {
            if hit {
                stats.record_hit();
            } else {
                stats.record_miss();
            }
        }
    }

    /// Whether the whole expression tree is promised to hit the columnar path.
    ///
    /// True only when every node is supported by the columnar evaluator and
    /// every `{var}.{prop}` property access has a matching compound slot in
    /// this chunk's layout. Debug-only: a miss for such an expression means
    /// the flat-column promise (T5) was broken and would silently regress
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
        let rows = vec![
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
                Value::BigInt(30),
                Value::string("Alice"),
            ],
        ];
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

    /// T5: attached counters record one hit per successful columnar
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

    /// T4.2: `get_column` lazily materialises and caches the columnar
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
}
