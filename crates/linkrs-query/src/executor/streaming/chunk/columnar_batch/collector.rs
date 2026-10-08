use std::cmp::Ordering;

use crate::executor::streaming::chunk::core::DataChunk;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use super::BatchColumn;

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
