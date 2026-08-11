//! Columnar batch accumulation for blocking operators
//!
//! Blocking operators (Sort/TopN/Aggregate) buffer all input before
//! producing output. Instead of buffering `Vec<Vec<Value>>` rows, rows are
//! accumulated column-major: homogeneous columns stay raw
//! (`Vec<i64>`/`Vec<f64>`/`Vec<i32>`/`Vec<bool>`/`Vec<i64>` days/`Vec<Arc<str>>`)
//! so that ordering/aggregation operate on scalars without constructing one
//! `Value` per row. Columns that mix kinds (or hit NULLs) degrade to
//! [`BatchColumn::Fallback`], keeping the exact `Value` semantics of the
//! row-based path.

use std::cmp::Ordering;
use std::sync::Arc;

use crate::core::value::date_time::DateValue;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::streaming::chunk::core::DataChunk;
use crate::query::executor::streaming::chunk::typed::{TypedColumn, TypedKind};
use crate::query::executor::streaming::helpers::compare_values;

/// Column-major accumulation of one output column across chunks.
#[derive(Debug, Clone)]
pub enum BatchColumn {
    /// No rows appended yet; the concrete kind is fixed by the first append.
    Empty,
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
    /// Days since epoch per row (see [`DateValue::to_days`]).
    Date(Vec<i64>),
    /// String column stored as `Vec<Arc<str>>`, avoiding per-row `Value` boxing.
    Utf8(Vec<Arc<str>>),
    /// Mixed-kind or NULL-bearing column; value-level semantics preserved.
    Fallback(Vec<Value>),
}

impl BatchColumn {
    pub fn len(&self) -> usize {
        match self {
            BatchColumn::Empty => 0,
            BatchColumn::I64(v) => v.len(),
            BatchColumn::F64(v) => v.len(),
            BatchColumn::I32(v) => v.len(),
            BatchColumn::Bool(v) => v.len(),
            BatchColumn::Date(v) => v.len(),
            BatchColumn::Utf8(v) => v.len(),
            BatchColumn::Fallback(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether this column uses a typed (non-fallback) representation.
    pub fn is_typed(&self) -> bool {
        !matches!(self, BatchColumn::Empty | BatchColumn::Fallback(_))
    }

    /// The raw kind of a typed column (None for Empty/Fallback).
    pub fn kind(&self) -> Option<TypedKind> {
        match self {
            BatchColumn::Empty | BatchColumn::Fallback(_) => None,
            BatchColumn::I64(_) => Some(TypedKind::I64),
            BatchColumn::F64(_) => Some(TypedKind::F64),
            BatchColumn::I32(_) => Some(TypedKind::I32),
            BatchColumn::Bool(_) => Some(TypedKind::Bool),
            BatchColumn::Date(_) => Some(TypedKind::Date),
            BatchColumn::Utf8(_) => Some(TypedKind::Utf8),
        }
    }

    /// Materialize the value at `idx` (O(1) for typed variants).
    pub fn value_at(&self, idx: usize) -> Value {
        match self {
            BatchColumn::Empty => Value::Null(NullType::Null),
            BatchColumn::I64(v) => Value::BigInt(v[idx]),
            BatchColumn::F64(v) => Value::Double(v[idx]),
            BatchColumn::I32(v) => Value::Int(v[idx]),
            BatchColumn::Bool(v) => Value::Bool(v[idx]),
            BatchColumn::Date(v) => Value::Date(DateValue::from_days(v[idx])),
            BatchColumn::Utf8(v) => Value::String(v[idx].as_ref().into()),
            BatchColumn::Fallback(v) => v[idx].clone(),
        }
    }

    /// Compare two rows on this column.
    ///
    /// Typed columns compare on raw scalars (identical ordering to
    /// [`compare_values`] for same-kind values: i64/i32/bool use the
    /// primitive order, f64 mirrors `Value` float ordering, strings are
    /// lexicographic). Date columns and fallback columns delegate to
    /// [`compare_values`] (the row path falls back to the string
    /// representation there, which diverges from the day order for
    /// pre-epoch dates).
    pub fn compare_at(&self, a: usize, b: usize) -> Ordering {
        match self {
            BatchColumn::Empty => Ordering::Equal,
            BatchColumn::I64(v) => v[a].cmp(&v[b]),
            BatchColumn::F64(v) => {
                let x = v[a];
                let y = v[b];
                if x < y {
                    Ordering::Less
                } else if x > y {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }
            BatchColumn::I32(v) => v[a].cmp(&v[b]),
            BatchColumn::Bool(v) => v[a].cmp(&v[b]),
            BatchColumn::Date(v) => compare_values(
                &Value::Date(DateValue::from_days(v[a])),
                &Value::Date(DateValue::from_days(v[b])),
            ),
            BatchColumn::Utf8(v) => v[a].cmp(&v[b]),
            BatchColumn::Fallback(v) => compare_values(&v[a], &v[b]),
        }
    }

    /// Compare the value `v` against row `idx` of this column.
    ///
    /// Uses the raw fast path when `v` matches the typed column kind;
    /// otherwise delegates to [`compare_values`] exactly.
    pub fn compare_value_at(&self, v: &Value, idx: usize) -> Ordering {
        let raw = match (self, v) {
            (BatchColumn::I64(col), Value::BigInt(x)) => Some(x.cmp(&col[idx])),
            (BatchColumn::F64(col), Value::Double(x)) => {
                let y = col[idx];
                Some(if *x < y {
                    Ordering::Less
                } else if *x > y {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                })
            }
            (BatchColumn::I32(col), Value::Int(x)) => Some(x.cmp(&col[idx])),
            (BatchColumn::Bool(col), Value::Bool(x)) => Some(x.cmp(&col[idx])),
            (BatchColumn::Utf8(col), Value::String(x)) => Some(x.as_str().cmp(&col[idx])),
            _ => None,
        };
        match raw {
            Some(ordering) => ordering,
            None => compare_values(v, &self.value_at(idx)),
        }
    }

    /// Estimated heap bytes of this column (for memory accounting).
    pub fn estimated_size(&self) -> usize {
        match self {
            BatchColumn::Empty => 0,
            BatchColumn::I64(v) => v.capacity() * std::mem::size_of::<i64>(),
            BatchColumn::F64(v) => v.capacity() * std::mem::size_of::<f64>(),
            BatchColumn::I32(v) => v.capacity() * std::mem::size_of::<i32>(),
            BatchColumn::Bool(v) => v.capacity() * std::mem::size_of::<bool>(),
            BatchColumn::Date(v) => v.capacity() * std::mem::size_of::<i64>(),
            BatchColumn::Utf8(v) => v.iter().map(|s| s.len()).sum(),
            BatchColumn::Fallback(v) => v.iter().map(Value::estimated_size).sum(),
        }
    }

    /// Append the entries of a chunk column at `indices`.
    ///
    /// When `self` is Empty the kind is taken from the chunk column (a
    /// fallback chunk column starts a fallback batch column). A kind
    /// mismatch degrades the accumulated column to [`BatchColumn::Fallback`].
    fn append_typed(&mut self, col: &TypedColumn, indices: &[usize]) {
        match self {
            BatchColumn::Empty => {
                *self = Self::gather(col, indices);
            }
            BatchColumn::I64(buf) => {
                if let TypedColumn::I64(src) = col {
                    buf.extend(indices.iter().map(|&i| src[i]));
                } else {
                    *self = Self::degraded_append(self, col, indices);
                }
            }
            BatchColumn::F64(buf) => {
                if let TypedColumn::F64(src) = col {
                    buf.extend(indices.iter().map(|&i| src[i]));
                } else {
                    *self = Self::degraded_append(self, col, indices);
                }
            }
            BatchColumn::I32(buf) => {
                if let TypedColumn::I32(src) = col {
                    buf.extend(indices.iter().map(|&i| src[i]));
                } else {
                    *self = Self::degraded_append(self, col, indices);
                }
            }
            BatchColumn::Bool(buf) => {
                if let TypedColumn::Bool(src) = col {
                    buf.extend(indices.iter().map(|&i| src[i]));
                } else {
                    *self = Self::degraded_append(self, col, indices);
                }
            }
            BatchColumn::Date(buf) => {
                if let TypedColumn::Date(src) = col {
                    buf.extend(indices.iter().map(|&i| src[i]));
                } else {
                    *self = Self::degraded_append(self, col, indices);
                }
            }
            BatchColumn::Utf8(buf) => {
                if let TypedColumn::Utf8(src) = col {
                    buf.extend(indices.iter().map(|&i| src[i].clone()));
                } else {
                    *self = Self::degraded_append(self, col, indices);
                }
            }
            BatchColumn::Fallback(buf) => {
                buf.extend(indices.iter().map(|&i| {
                    col.value_at(i)
                        .unwrap_or_else(|| Value::Null(NullType::Null))
                }));
            }
        }
    }

    /// Build a batch column from a chunk column at `indices` (kind taken
    /// from the chunk column).
    fn gather(col: &TypedColumn, indices: &[usize]) -> Self {
        match col {
            TypedColumn::I64(v) => BatchColumn::I64(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::F64(v) => BatchColumn::F64(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::I32(v) => BatchColumn::I32(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::Bool(v) => BatchColumn::Bool(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::Date(v) => BatchColumn::Date(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::Utf8(v) => {
                BatchColumn::Utf8(indices.iter().map(|&i| v[i].clone()).collect())
            }
            TypedColumn::Fallback(v) => {
                BatchColumn::Fallback(indices.iter().map(|&i| v[i].clone()).collect())
            }
        }
    }

    /// Degrade an existing typed column to `Fallback` (keeping accumulated
    /// rows) and append the chunk column values.
    fn degraded_append(current: &Self, col: &TypedColumn, indices: &[usize]) -> Self {
        let mut values: Vec<Value> = (0..current.len()).map(|i| current.value_at(i)).collect();
        values.extend(indices.iter().map(|&i| {
            col.value_at(i)
                .unwrap_or_else(|| Value::Null(NullType::Null))
        }));
        BatchColumn::Fallback(values)
    }

    fn append_row_value(&mut self, value: &Value) {
        match self {
            BatchColumn::Empty => {
                // No kind established yet: start as a single-value fallback
                // (rows do not carry typed raw data on the append path).
                *self = BatchColumn::Fallback(vec![value.clone()]);
            }
            BatchColumn::I64(buf) => {
                if let Value::BigInt(x) = value {
                    buf.push(*x);
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::F64(buf) => {
                if let Value::Double(x) = value {
                    buf.push(*x);
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::I32(buf) => {
                if let Value::Int(x) = value {
                    buf.push(*x);
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::Bool(buf) => {
                if let Value::Bool(x) = value {
                    buf.push(*x);
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::Date(buf) => {
                if let Value::Date(x) = value {
                    buf.push(x.to_days());
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::Utf8(buf) => {
                if let Value::String(x) = value {
                    buf.push(Arc::from(x.as_str()));
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::Fallback(buf) => buf.push(value.clone()),
        }
    }

    fn degraded_push(current: &Self, value: &Value) -> Self {
        let mut values: Vec<Value> = (0..current.len()).map(|i| current.value_at(i)).collect();
        values.push(value.clone());
        BatchColumn::Fallback(values)
    }

    fn truncate(&mut self, len: usize) {
        match self {
            BatchColumn::Empty => {}
            BatchColumn::I64(v) => v.truncate(len),
            BatchColumn::F64(v) => v.truncate(len),
            BatchColumn::I32(v) => v.truncate(len),
            BatchColumn::Bool(v) => v.truncate(len),
            BatchColumn::Date(v) => v.truncate(len),
            BatchColumn::Utf8(v) => v.truncate(len),
            BatchColumn::Fallback(v) => v.truncate(len),
        }
    }

    fn permute(&mut self, perm: &[usize]) {
        match self {
            BatchColumn::Empty => {}
            BatchColumn::I64(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i]).collect();
            }
            BatchColumn::F64(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i]).collect();
            }
            BatchColumn::I32(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i]).collect();
            }
            BatchColumn::Bool(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i]).collect();
            }
            BatchColumn::Date(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i]).collect();
            }
            BatchColumn::Utf8(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i].clone()).collect();
            }
            BatchColumn::Fallback(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i].clone()).collect();
            }
        }
    }
}

/// Column-major accumulation of a full relation (one [`BatchColumn`] per
/// output column), used by blocking operators below the spill boundary.
#[derive(Debug, Clone, Default)]
pub struct ColumnarBatch {
    columns: Vec<BatchColumn>,
    num_rows: usize,
}

impl ColumnarBatch {
    /// Create an empty batch with `num_columns` columns.
    pub fn new(num_columns: usize) -> Self {
        Self {
            columns: vec![BatchColumn::Empty; num_columns],
            num_rows: 0,
        }
    }

    pub fn num_columns(&self) -> usize {
        self.columns.len()
    }

    pub fn num_rows(&self) -> usize {
        self.num_rows
    }

    pub fn is_empty(&self) -> bool {
        self.num_rows == 0
    }

    pub fn column(&self, idx: usize) -> &BatchColumn {
        &self.columns[idx]
    }

    /// Append all visible rows of `chunk`.
    ///
    /// Each column takes its raw kind from the chunk's typed layout; a kind
    /// mismatch (or a fallback chunk column) degrades that column to
    /// [`BatchColumn::Fallback`] with the accumulated rows preserved.
    pub fn append_chunk(&mut self, chunk: &DataChunk) -> usize {
        let indices = chunk.visible_indices();
        if indices.is_empty() {
            return self.num_rows;
        }
        let num_cols = chunk.num_columns();
        if self.columns.is_empty() {
            self.columns = vec![BatchColumn::Empty; num_cols];
        }
        if self.columns.len() < num_cols {
            self.columns.resize(num_cols, BatchColumn::Empty);
        }
        for j in 0..num_cols {
            let column = &mut self.columns[j];
            if let Some(typed) = chunk.typed_column(j) {
                column.append_typed(typed, &indices);
            } else {
                // Row-based chunk: per-value append.
                let is_typed = column.is_typed();
                if is_typed {
                    for &i in &indices {
                        let value = chunk
                            .rows
                            .get(i)
                            .and_then(|r| r.get(j))
                            .cloned()
                            .unwrap_or_else(|| Value::Null(NullType::Null));
                        column.append_row_value(&value);
                    }
                } else {
                    for &i in &indices {
                        let value = chunk
                            .rows
                            .get(i)
                            .and_then(|r| r.get(j))
                            .cloned()
                            .unwrap_or_else(|| Value::Null(NullType::Null));
                        if matches!(column, BatchColumn::Empty) {
                            *column = BatchColumn::Fallback(vec![value]);
                        } else {
                            column.append_row_value(&value);
                        }
                    }
                }
            }
        }
        self.num_rows += indices.len();
        self.num_rows
    }

    /// Append a single visible row of `chunk` at `idx`.
    ///
    /// Keeps the raw typed fast path per column (same degradation rules as
    /// [`Self::append_chunk`]); used by operators that account memory per
    /// appended row before buffering.
    pub fn append_chunk_row(&mut self, chunk: &DataChunk, idx: usize) {
        let num_cols = chunk.num_columns();
        if self.columns.is_empty() {
            self.columns = vec![BatchColumn::Empty; num_cols];
        }
        if self.columns.len() < num_cols {
            self.columns.resize(num_cols, BatchColumn::Empty);
        }
        for (j, column) in self.columns.iter_mut().enumerate().take(num_cols) {
            if let Some(typed) = chunk.typed_column(j) {
                column.append_typed(typed, std::slice::from_ref(&idx));
            } else {
                let value = chunk
                    .rows
                    .get(idx)
                    .and_then(|r| r.get(j))
                    .cloned()
                    .unwrap_or_else(|| Value::Null(NullType::Null));
                if matches!(column, BatchColumn::Empty) {
                    *column = BatchColumn::Fallback(vec![value]);
                } else {
                    column.append_row_value(&value);
                }
            }
        }
        self.num_rows += 1;
    }

    /// Append a single row of values (fallback path).
    pub fn append_row(&mut self, row: &[Value]) {
        if self.columns.is_empty() {
            self.columns = vec![BatchColumn::Empty; row.len()];
        }
        if self.columns.len() < row.len() {
            self.columns.resize(row.len(), BatchColumn::Empty);
        }
        for (j, column) in self.columns.iter_mut().enumerate() {
            let value = row
                .get(j)
                .cloned()
                .unwrap_or_else(|| Value::Null(NullType::Null));
            column.append_row_value(&value);
        }
        self.num_rows += 1;
    }

    /// Materialize rows (row-major) from the current columnar state.
    pub fn to_rows(&self) -> Vec<Vec<Value>> {
        let mut rows = Vec::with_capacity(self.num_rows);
        for i in 0..self.num_rows {
            let mut row = Vec::with_capacity(self.columns.len());
            for column in &self.columns {
                row.push(column.value_at(i));
            }
            rows.push(row);
        }
        rows
    }

    /// Compare rows `a` and `b` on column `col` (raw fast path when typed).
    pub fn compare_rows_at(&self, col: usize, a: usize, b: usize) -> Ordering {
        self.columns[col].compare_at(a, b)
    }

    /// Compare the value `v` against row `idx` on column `col`.
    pub fn compare_value_at(&self, col: usize, v: &Value, idx: usize) -> Ordering {
        self.columns[col].compare_value_at(v, idx)
    }

    /// Reorder rows by `perm` (`result[i] = old[perm[i]]`).
    pub fn permute(&mut self, perm: &[usize]) {
        for column in &mut self.columns {
            column.permute(perm);
        }
    }

    /// Keep only the first `len` rows (columns must already be in the
    /// desired order).
    pub fn truncate(&mut self, len: usize) {
        for column in &mut self.columns {
            column.truncate(len);
        }
        self.num_rows = len.min(self.num_rows);
    }

    /// Drop all accumulated rows.
    pub fn clear(&mut self) {
        for column in &mut self.columns {
            *column = BatchColumn::Empty;
        }
        self.num_rows = 0;
    }

    /// Estimated heap bytes (for memory accounting).
    pub fn estimated_size(&self) -> usize {
        self.columns.iter().map(|c| c.estimated_size()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::chunk::schema::{ColumnInfo, Schema};

    fn chunk_of(rows: Vec<Vec<Value>>) -> DataChunk {
        let mut chunk = DataChunk::new(
            rows,
            Arc::new(Schema::new(vec![ColumnInfo {
                name: "val".to_string(),
                data_type: "bigint".to_string(),
            }])),
        );
        chunk.build_typed_columns(true);
        chunk
    }

    #[test]
    fn test_append_chunk_typed_i64() {
        let mut batch = ColumnarBatch::new(1);
        let chunk = chunk_of(vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
            vec![Value::BigInt(3)],
        ]);
        batch.append_chunk(&chunk);
        assert_eq!(batch.num_rows(), 3);
        assert!(batch.column(0).is_typed());
        assert_eq!(batch.column(0).value_at(1), Value::BigInt(2));
        assert_eq!(batch.column(0).compare_at(0, 2), Ordering::Less);
        assert_eq!(
            batch.to_rows(),
            vec![
                vec![Value::BigInt(1)],
                vec![Value::BigInt(2)],
                vec![Value::BigInt(3)],
            ]
        );
    }

    #[test]
    fn test_append_chunk_degrades_on_kind_mismatch() {
        let mut batch = ColumnarBatch::new(1);
        let c1 = chunk_of(vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]]);
        batch.append_chunk(&c1);
        assert!(batch.column(0).is_typed());
        // A later chunk carries an Int (typed I32) in the same column:
        // degrade to Fallback, preserving accumulated values.
        let mut c2 = DataChunk::new(
            vec![vec![Value::Int(7)], vec![Value::Int(8)]],
            Arc::new(Schema::new(vec![ColumnInfo {
                name: "val".to_string(),
                data_type: "int".to_string(),
            }])),
        );
        c2.build_typed_columns(true);
        batch.append_chunk(&c2);
        assert!(!batch.column(0).is_typed());
        assert_eq!(batch.num_rows(), 4);
        let rows = batch.to_rows();
        assert_eq!(rows[0][0], Value::BigInt(1));
        assert_eq!(rows[2][0], Value::Int(7));
        assert_eq!(rows[3][0], Value::Int(8));
    }

    #[test]
    fn test_permute_and_truncate() {
        let mut batch = ColumnarBatch::new(1);
        batch.append_chunk(&chunk_of(vec![
            vec![Value::BigInt(3)],
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
        ]));
        // perm[i] = source index for position i → ascending order.
        batch.permute(&[1, 2, 0]);
        assert_eq!(batch.column(0).value_at(0), Value::BigInt(1));
        assert_eq!(batch.column(0).value_at(1), Value::BigInt(2));
        assert_eq!(batch.column(0).value_at(2), Value::BigInt(3));
        batch.truncate(2);
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.to_rows()[1][0], Value::BigInt(2));
    }

    #[test]
    fn test_utf8_column_ordering() {
        let mut batch = ColumnarBatch::new(1);
        batch.append_chunk(&chunk_of(vec![
            vec![Value::String("pear".into())],
            vec![Value::String("apple".into())],
            vec![Value::String("fig".into())],
        ]));
        assert!(batch.column(0).is_typed());
        assert_eq!(
            batch.column(0).compare_at(1, 0),
            Ordering::Less,
            "apple < pear"
        );
        assert_eq!(
            batch.column(0).compare_at(2, 0),
            Ordering::Less,
            "fig < pear"
        );
    }

    #[test]
    fn test_compare_value_at_mixed_kind_falls_back() {
        let mut batch = ColumnarBatch::new(1);
        batch.append_chunk(&chunk_of(vec![vec![Value::BigInt(100)]]));
        // Int(5) vs BigInt(100): cross-type compare falls back to
        // compare_values semantics.
        assert_eq!(
            batch.compare_value_at(0, &Value::Int(5), 0),
            compare_values(&Value::Int(5), &Value::BigInt(100))
        );
        // Same-kind BigInt uses the raw fast path.
        assert_eq!(
            batch.compare_value_at(0, &Value::BigInt(50), 0),
            Ordering::Less
        );
    }

    #[test]
    fn test_estimated_size() {
        let mut batch = ColumnarBatch::new(1);
        let chunk = chunk_of(vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
            vec![Value::BigInt(3)],
        ]);
        batch.append_chunk(&chunk);
        assert!(batch.estimated_size() >= 3 * std::mem::size_of::<i64>());
    }
}
