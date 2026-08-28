//! Pool of recycled row/column buffers for DataChunk construction.

use super::typed::{TypedColumn, TypedKind};
use graphdb_core::value::decimal128::Decimal128Value;
use graphdb_core::Value;
use std::sync::Arc;

const ROW_POOL_MAX_SIZE: usize = 8;

/// Pool of recycled `Vec<Vec<Value>>` allocations for DataChunk construction.
///
/// Reduces allocation overhead by reusing Vec buffers across chunk boundaries.
/// Each acquired Vec is guaranteed to have `chunk_size` capacity (not length).
/// Typed allocation pools (`Vec<i64>`/`Vec<f64>`/`Vec<i32>`/`Vec<bool>`/
/// `Vec<i64>` for dates and date-times/`Vec<Arc<str>>` for strings/
/// `Vec<Decimal128Value>` for decimals) recycle typed column buffers for
/// `TypedColumn` construction.
pub struct RowPool {
    pool: parking_lot::Mutex<Vec<Vec<Vec<Value>>>>,
    typed_i64: parking_lot::Mutex<Vec<Vec<i64>>>,
    typed_f64: parking_lot::Mutex<Vec<Vec<f64>>>,
    typed_i32: parking_lot::Mutex<Vec<Vec<i32>>>,
    typed_bool: parking_lot::Mutex<Vec<Vec<bool>>>,
    typed_date: parking_lot::Mutex<Vec<Vec<i64>>>,
    typed_datetime: parking_lot::Mutex<Vec<Vec<i64>>>,
    typed_utf8: parking_lot::Mutex<Vec<Vec<Arc<str>>>>,
    typed_decimal: parking_lot::Mutex<Vec<Vec<Decimal128Value>>>,
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
            typed_bool: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
            typed_date: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
            typed_datetime: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
            typed_utf8: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
            typed_decimal: parking_lot::Mutex::new(Vec::with_capacity(ROW_POOL_MAX_SIZE)),
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
            TypedKind::Bool => {
                let mut p = self.typed_bool.lock();
                if let Some(mut buf) = p.pop() {
                    buf.clear();
                    TypedColumn::Bool(buf)
                } else {
                    TypedColumn::Bool(Vec::with_capacity(cap))
                }
            }
            TypedKind::Date => {
                let mut p = self.typed_date.lock();
                if let Some(mut buf) = p.pop() {
                    buf.clear();
                    TypedColumn::Date(buf)
                } else {
                    TypedColumn::Date(Vec::with_capacity(cap))
                }
            }
            TypedKind::DateTime => {
                let mut p = self.typed_datetime.lock();
                if let Some(mut buf) = p.pop() {
                    buf.clear();
                    TypedColumn::DateTime(buf)
                } else {
                    TypedColumn::DateTime(Vec::with_capacity(cap))
                }
            }
            TypedKind::Utf8 => {
                let mut p = self.typed_utf8.lock();
                if let Some(mut buf) = p.pop() {
                    buf.clear();
                    TypedColumn::Utf8(buf)
                } else {
                    TypedColumn::Utf8(Vec::with_capacity(cap))
                }
            }
            TypedKind::Decimal => {
                let mut p = self.typed_decimal.lock();
                if let Some(mut buf) = p.pop() {
                    buf.clear();
                    TypedColumn::Decimal(buf)
                } else {
                    TypedColumn::Decimal(Vec::with_capacity(cap))
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
            TypedColumn::Bool(mut buf) => {
                buf.clear();
                let mut p = self.typed_bool.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::Date(mut buf) => {
                buf.clear();
                let mut p = self.typed_date.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::DateTime(mut buf) => {
                buf.clear();
                let mut p = self.typed_datetime.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::Utf8(mut buf) => {
                buf.clear();
                let mut p = self.typed_utf8.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::Decimal(mut buf) => {
                buf.clear();
                let mut p = self.typed_decimal.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            // Nullable variants recycle the values buffer (the validity
            // bitmap is dropped and rebuilt by the chunk builder).
            TypedColumn::NullableI64(mut buf, _) => {
                buf.clear();
                let mut p = self.typed_i64.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::NullableF64(mut buf, _) => {
                buf.clear();
                let mut p = self.typed_f64.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::NullableI32(mut buf, _) => {
                buf.clear();
                let mut p = self.typed_i32.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::NullableBool(mut buf, _) => {
                buf.clear();
                let mut p = self.typed_bool.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::NullableDate(mut buf, _) => {
                buf.clear();
                let mut p = self.typed_date.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::NullableDateTime(mut buf, _) => {
                buf.clear();
                let mut p = self.typed_datetime.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::NullableUtf8(mut buf, _) => {
                buf.clear();
                let mut p = self.typed_utf8.lock();
                if p.len() < ROW_POOL_MAX_SIZE {
                    p.push(buf);
                }
            }
            TypedColumn::NullableDecimal(mut buf, _) => {
                buf.clear();
                let mut p = self.typed_decimal.lock();
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
