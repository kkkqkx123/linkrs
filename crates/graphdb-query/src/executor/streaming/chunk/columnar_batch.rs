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

use crate::executor::streaming::chunk::core::DataChunk;
use crate::executor::streaming::chunk::typed::{bitmap_is_valid, TypedColumn, TypedKind};
use crate::executor::streaming::helpers::compare_values;
use graphdb_core::value::date_time::{DateTimeValue, DateValue};
use graphdb_core::value::decimal128::Decimal128Value;
use graphdb_core::value::NullType;
use graphdb_core::Value;

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
    /// Micros since epoch per row (see [`DateTimeValue::to_micros`]).
    DateTime(Vec<i64>),
    /// String column stored as `Vec<Arc<str>>`, avoiding per-row `Value` boxing.
    Utf8(Vec<Arc<str>>),
    /// Decimal128 per row (decimal semantics, `Ord`).
    Decimal(Vec<Decimal128Value>),
    /// Typed column with a validity bitmap (`1` = valid, `0` = NULL).
    /// Invalid rows materialize as NULL and sort last.
    NullableI64(Vec<i64>, Vec<u64>),
    NullableF64(Vec<f64>, Vec<u64>),
    NullableI32(Vec<i32>, Vec<u64>),
    NullableBool(Vec<bool>, Vec<u64>),
    NullableDate(Vec<i64>, Vec<u64>),
    NullableDateTime(Vec<i64>, Vec<u64>),
    NullableUtf8(Vec<Arc<str>>, Vec<u64>),
    NullableDecimal(Vec<Decimal128Value>, Vec<u64>),
    /// Mixed-kind or NULL-bearing column; value-level semantics preserved.
    Fallback(Vec<Value>),
}

/// Compare two rows of a nullable column with NULL-last ordering (mirrors
/// [`compare_values`]: NULL equals NULL and sorts last).
fn nullable_cmp_at<T>(
    bitmap: &[u64],
    a: &T,
    b: &T,
    a_idx: usize,
    b_idx: usize,
    cmp: fn(&T, &T) -> Ordering,
) -> Ordering {
    let a_valid = bitmap_is_valid(bitmap, a_idx);
    let b_valid = bitmap_is_valid(bitmap, b_idx);
    match (a_valid, b_valid) {
        (true, true) => cmp(a, b),
        (false, false) => Ordering::Equal,
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
    }
}

/// Append validity bits (packed, one bit per row) to a bitmap starting at
/// row `rows_before`.
fn extend_bitmap(bm: &mut Vec<u64>, rows_before: usize, valid: impl Iterator<Item = bool>) {
    for (row, is_valid) in (rows_before..).zip(valid) {
        let word = row / 64;
        if word >= bm.len() {
            bm.resize(word + 1, 0u64);
        }
        if is_valid {
            bm[word] |= 1u64 << (row % 64);
        }
    }
}

/// Build a packed validity bitmap marking the rows at `indices` that are
/// valid in the source bitmap.
fn bitmap_from_indices(bitmap: &[u64], indices: &[usize]) -> Vec<u64> {
    let mut out = vec![0u64; indices.len().div_ceil(64)];
    for (j, &i) in indices.iter().enumerate() {
        if bitmap_is_valid(bitmap, i) {
            out[j / 64] |= 1u64 << (j % 64);
        }
    }
    out
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
            BatchColumn::DateTime(v) => v.len(),
            BatchColumn::Utf8(v) => v.len(),
            BatchColumn::Decimal(v) => v.len(),
            BatchColumn::NullableI64(v, _) => v.len(),
            BatchColumn::NullableF64(v, _) => v.len(),
            BatchColumn::NullableI32(v, _) => v.len(),
            BatchColumn::NullableBool(v, _) => v.len(),
            BatchColumn::NullableDate(v, _) => v.len(),
            BatchColumn::NullableDateTime(v, _) => v.len(),
            BatchColumn::NullableUtf8(v, _) => v.len(),
            BatchColumn::NullableDecimal(v, _) => v.len(),
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
            BatchColumn::I64(_) | BatchColumn::NullableI64(..) => Some(TypedKind::I64),
            BatchColumn::F64(_) | BatchColumn::NullableF64(..) => Some(TypedKind::F64),
            BatchColumn::I32(_) | BatchColumn::NullableI32(..) => Some(TypedKind::I32),
            BatchColumn::Bool(_) | BatchColumn::NullableBool(..) => Some(TypedKind::Bool),
            BatchColumn::Date(_) | BatchColumn::NullableDate(..) => Some(TypedKind::Date),
            BatchColumn::DateTime(_) | BatchColumn::NullableDateTime(..) => {
                Some(TypedKind::DateTime)
            }
            BatchColumn::Utf8(_) | BatchColumn::NullableUtf8(..) => Some(TypedKind::Utf8),
            BatchColumn::Decimal(_) | BatchColumn::NullableDecimal(..) => Some(TypedKind::Decimal),
        }
    }

    /// Materialize the value at `idx` (O(1) for typed variants; NULL for
    /// invalid rows of the `Nullable*` variants).
    pub fn value_at(&self, idx: usize) -> Value {
        match self {
            BatchColumn::Empty => Value::Null(NullType::Null),
            BatchColumn::I64(v) => Value::BigInt(v[idx]),
            BatchColumn::F64(v) => Value::Double(v[idx]),
            BatchColumn::I32(v) => Value::Int(v[idx]),
            BatchColumn::Bool(v) => Value::Bool(v[idx]),
            BatchColumn::Date(v) => Value::Date(DateValue::from_days(v[idx])),
            BatchColumn::DateTime(v) => Value::DateTime(DateTimeValue::from_micros(v[idx])),
            BatchColumn::Utf8(v) => Value::String(v[idx].as_ref().into()),
            BatchColumn::Decimal(v) => Value::Decimal128(v[idx].clone()),
            BatchColumn::NullableI64(v, b) => {
                if bitmap_is_valid(b, idx) {
                    Value::BigInt(v[idx])
                } else {
                    Value::Null(NullType::Null)
                }
            }
            BatchColumn::NullableF64(v, b) => {
                if bitmap_is_valid(b, idx) {
                    Value::Double(v[idx])
                } else {
                    Value::Null(NullType::Null)
                }
            }
            BatchColumn::NullableI32(v, b) => {
                if bitmap_is_valid(b, idx) {
                    Value::Int(v[idx])
                } else {
                    Value::Null(NullType::Null)
                }
            }
            BatchColumn::NullableBool(v, b) => {
                if bitmap_is_valid(b, idx) {
                    Value::Bool(v[idx])
                } else {
                    Value::Null(NullType::Null)
                }
            }
            BatchColumn::NullableDate(v, b) => {
                if bitmap_is_valid(b, idx) {
                    Value::Date(DateValue::from_days(v[idx]))
                } else {
                    Value::Null(NullType::Null)
                }
            }
            BatchColumn::NullableDateTime(v, b) => {
                if bitmap_is_valid(b, idx) {
                    Value::DateTime(DateTimeValue::from_micros(v[idx]))
                } else {
                    Value::Null(NullType::Null)
                }
            }
            BatchColumn::NullableUtf8(v, b) => {
                if bitmap_is_valid(b, idx) {
                    Value::String(v[idx].as_ref().into())
                } else {
                    Value::Null(NullType::Null)
                }
            }
            BatchColumn::NullableDecimal(v, b) => {
                if bitmap_is_valid(b, idx) {
                    Value::Decimal128(v[idx].clone())
                } else {
                    Value::Null(NullType::Null)
                }
            }
            BatchColumn::Fallback(v) => v[idx].clone(),
        }
    }

    /// Compare two rows on this column.
    ///
    /// Typed columns compare on raw scalars (identical ordering to
    /// [`compare_values`] for same-kind values: i64/i32/bool use the
    /// primitive order, f64 mirrors `Value` float ordering, strings are
    /// lexicographic). NULL ordering matches [`compare_values`] (NULLs last).
    /// Date columns and fallback columns delegate to [`compare_values`] (the
    /// row path falls back to the string representation there, which
    /// diverges from the day order for pre-epoch dates).
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
            BatchColumn::DateTime(v) => compare_values(
                &Value::DateTime(DateTimeValue::from_micros(v[a])),
                &Value::DateTime(DateTimeValue::from_micros(v[b])),
            ),
            BatchColumn::Utf8(v) => v[a].cmp(&v[b]),
            BatchColumn::Decimal(v) => v[a].cmp(&v[b]),
            BatchColumn::NullableI64(v, bm) => {
                nullable_cmp_at(bm, &v[a], &v[b], a, b, |x, y| x.cmp(y))
            }
            BatchColumn::NullableF64(v, bm) => nullable_cmp_at(bm, &v[a], &v[b], a, b, |x, y| {
                if x < y {
                    Ordering::Less
                } else if x > y {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }),
            BatchColumn::NullableI32(v, bm) => {
                nullable_cmp_at(bm, &v[a], &v[b], a, b, |x, y| x.cmp(y))
            }
            BatchColumn::NullableBool(v, bm) => {
                nullable_cmp_at(bm, &v[a], &v[b], a, b, |x, y| x.cmp(y))
            }
            BatchColumn::NullableDate(v, bm) => {
                nullable_cmp_at(bm, &v[a], &v[b], a, b, |x, y| x.cmp(y))
            }
            BatchColumn::NullableDateTime(v, bm) => {
                nullable_cmp_at(bm, &v[a], &v[b], a, b, |x, y| x.cmp(y))
            }
            BatchColumn::NullableUtf8(v, bm) => {
                nullable_cmp_at(bm, &v[a], &v[b], a, b, |x, y| x.cmp(y))
            }
            BatchColumn::NullableDecimal(v, bm) => {
                nullable_cmp_at(bm, &v[a], &v[b], a, b, |x, y| x.cmp(y))
            }
            BatchColumn::Fallback(v) => compare_values(&v[a], &v[b]),
        }
    }

    /// Compare the value `v` against row `idx` of this column.
    ///
    /// Uses the raw fast path when `v` matches the typed column kind and the
    /// row is valid; otherwise delegates to [`compare_values`] exactly.
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
            (BatchColumn::NullableI64(col, b), Value::BigInt(x)) => {
                bitmap_is_valid(b, idx).then(|| x.cmp(&col[idx]))
            }
            (BatchColumn::NullableF64(col, b), Value::Double(x)) => {
                bitmap_is_valid(b, idx).then(|| {
                    let y = col[idx];
                    if *x < y {
                        Ordering::Less
                    } else if *x > y {
                        Ordering::Greater
                    } else {
                        Ordering::Equal
                    }
                })
            }
            (BatchColumn::NullableI32(col, b), Value::Int(x)) => {
                bitmap_is_valid(b, idx).then(|| x.cmp(&col[idx]))
            }
            (BatchColumn::NullableBool(col, b), Value::Bool(x)) => {
                bitmap_is_valid(b, idx).then(|| x.cmp(&col[idx]))
            }
            (BatchColumn::NullableUtf8(col, b), Value::String(x)) => {
                bitmap_is_valid(b, idx).then(|| x.as_str().cmp(&col[idx]))
            }
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
            BatchColumn::DateTime(v) => v.capacity() * std::mem::size_of::<i64>(),
            BatchColumn::Utf8(v) => v.iter().map(|s| s.len()).sum(),
            BatchColumn::Decimal(v) => v.capacity() * std::mem::size_of::<Decimal128Value>(),
            BatchColumn::NullableI64(v, b) => {
                v.capacity() * std::mem::size_of::<i64>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            BatchColumn::NullableF64(v, b) => {
                v.capacity() * std::mem::size_of::<f64>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            BatchColumn::NullableI32(v, b) => {
                v.capacity() * std::mem::size_of::<i32>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            BatchColumn::NullableBool(v, b) => {
                v.capacity() * std::mem::size_of::<bool>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            BatchColumn::NullableDate(v, b) => {
                v.capacity() * std::mem::size_of::<i64>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            BatchColumn::NullableDateTime(v, b) => {
                v.capacity() * std::mem::size_of::<i64>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            BatchColumn::NullableUtf8(v, b) => {
                v.iter().map(|s| s.len()).sum::<usize>() + b.capacity() * std::mem::size_of::<u64>()
            }
            BatchColumn::NullableDecimal(v, b) => {
                v.capacity() * std::mem::size_of::<Decimal128Value>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            BatchColumn::Fallback(v) => v.iter().map(Value::estimated_size).sum(),
        }
    }

    /// Append the entries of a chunk column at `indices`.
    ///
    /// When `self` is Empty the kind is taken from the chunk column (a
    /// fallback chunk column starts a fallback batch column). A kind
    /// mismatch degrades the accumulated column to [`BatchColumn::Fallback`].
    /// A NULL introduced by a `Nullable*` chunk column upgrades a plain
    /// typed column to its `Nullable*` form (past rows become valid).
    fn append_typed(&mut self, col: &TypedColumn, indices: &[usize]) {
        match self {
            BatchColumn::Empty => {
                *self = Self::gather(col, indices);
            }
            BatchColumn::I64(buf) => match col {
                TypedColumn::I64(src) => buf.extend(indices.iter().map(|&i| src[i])),
                TypedColumn::NullableI64(src, bm) => {
                    let rows_before = buf.len();
                    *self = Self::to_nullable(self);
                    if let BatchColumn::NullableI64(buf, bm_out) = self {
                        buf.extend(indices.iter().map(|&i| src[i]));
                        extend_bitmap(
                            bm_out,
                            rows_before,
                            indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                        );
                    }
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::F64(buf) => match col {
                TypedColumn::F64(src) => buf.extend(indices.iter().map(|&i| src[i])),
                TypedColumn::NullableF64(src, bm) => {
                    let rows_before = buf.len();
                    *self = Self::to_nullable(self);
                    if let BatchColumn::NullableF64(buf, bm_out) = self {
                        buf.extend(indices.iter().map(|&i| src[i]));
                        extend_bitmap(
                            bm_out,
                            rows_before,
                            indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                        );
                    }
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::I32(buf) => match col {
                TypedColumn::I32(src) => buf.extend(indices.iter().map(|&i| src[i])),
                TypedColumn::NullableI32(src, bm) => {
                    let rows_before = buf.len();
                    *self = Self::to_nullable(self);
                    if let BatchColumn::NullableI32(buf, bm_out) = self {
                        buf.extend(indices.iter().map(|&i| src[i]));
                        extend_bitmap(
                            bm_out,
                            rows_before,
                            indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                        );
                    }
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::Bool(buf) => match col {
                TypedColumn::Bool(src) => buf.extend(indices.iter().map(|&i| src[i])),
                TypedColumn::NullableBool(src, bm) => {
                    let rows_before = buf.len();
                    *self = Self::to_nullable(self);
                    if let BatchColumn::NullableBool(buf, bm_out) = self {
                        buf.extend(indices.iter().map(|&i| src[i]));
                        extend_bitmap(
                            bm_out,
                            rows_before,
                            indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                        );
                    }
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::Date(buf) => match col {
                TypedColumn::Date(src) => buf.extend(indices.iter().map(|&i| src[i])),
                TypedColumn::NullableDate(src, bm) => {
                    let rows_before = buf.len();
                    *self = Self::to_nullable(self);
                    if let BatchColumn::NullableDate(buf, bm_out) = self {
                        buf.extend(indices.iter().map(|&i| src[i]));
                        extend_bitmap(
                            bm_out,
                            rows_before,
                            indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                        );
                    }
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::DateTime(buf) => match col {
                TypedColumn::DateTime(src) => buf.extend(indices.iter().map(|&i| src[i])),
                TypedColumn::NullableDateTime(src, bm) => {
                    let rows_before = buf.len();
                    *self = Self::to_nullable(self);
                    if let BatchColumn::NullableDateTime(buf, bm_out) = self {
                        buf.extend(indices.iter().map(|&i| src[i]));
                        extend_bitmap(
                            bm_out,
                            rows_before,
                            indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                        );
                    }
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::Utf8(buf) => match col {
                TypedColumn::Utf8(src) => buf.extend(indices.iter().map(|&i| src[i].clone())),
                TypedColumn::NullableUtf8(src, bm) => {
                    let rows_before = buf.len();
                    *self = Self::to_nullable(self);
                    if let BatchColumn::NullableUtf8(buf, bm_out) = self {
                        buf.extend(indices.iter().map(|&i| src[i].clone()));
                        extend_bitmap(
                            bm_out,
                            rows_before,
                            indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                        );
                    }
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::Decimal(buf) => match col {
                TypedColumn::Decimal(src) => buf.extend(indices.iter().map(|&i| src[i].clone())),
                TypedColumn::NullableDecimal(src, bm) => {
                    let rows_before = buf.len();
                    *self = Self::to_nullable(self);
                    if let BatchColumn::NullableDecimal(buf, bm_out) = self {
                        buf.extend(indices.iter().map(|&i| src[i].clone()));
                        extend_bitmap(
                            bm_out,
                            rows_before,
                            indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                        );
                    }
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::NullableI64(buf, bm_out) => match col {
                TypedColumn::NullableI64(src, bm) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(
                        bm_out,
                        rows_before,
                        indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                    );
                }
                TypedColumn::I64(src) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(bm_out, rows_before, indices.iter().map(|_| true));
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::NullableF64(buf, bm_out) => match col {
                TypedColumn::NullableF64(src, bm) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(
                        bm_out,
                        rows_before,
                        indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                    );
                }
                TypedColumn::F64(src) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(bm_out, rows_before, indices.iter().map(|_| true));
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::NullableI32(buf, bm_out) => match col {
                TypedColumn::NullableI32(src, bm) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(
                        bm_out,
                        rows_before,
                        indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                    );
                }
                TypedColumn::I32(src) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(bm_out, rows_before, indices.iter().map(|_| true));
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::NullableBool(buf, bm_out) => match col {
                TypedColumn::NullableBool(src, bm) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(
                        bm_out,
                        rows_before,
                        indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                    );
                }
                TypedColumn::Bool(src) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(bm_out, rows_before, indices.iter().map(|_| true));
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::NullableDate(buf, bm_out) => match col {
                TypedColumn::NullableDate(src, bm) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(
                        bm_out,
                        rows_before,
                        indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                    );
                }
                TypedColumn::Date(src) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(bm_out, rows_before, indices.iter().map(|_| true));
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::NullableDateTime(buf, bm_out) => match col {
                TypedColumn::NullableDateTime(src, bm) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(
                        bm_out,
                        rows_before,
                        indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                    );
                }
                TypedColumn::DateTime(src) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i]));
                    extend_bitmap(bm_out, rows_before, indices.iter().map(|_| true));
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::NullableUtf8(buf, bm_out) => match col {
                TypedColumn::NullableUtf8(src, bm) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i].clone()));
                    extend_bitmap(
                        bm_out,
                        rows_before,
                        indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                    );
                }
                TypedColumn::Utf8(src) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i].clone()));
                    extend_bitmap(bm_out, rows_before, indices.iter().map(|_| true));
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
            BatchColumn::NullableDecimal(buf, bm_out) => match col {
                TypedColumn::NullableDecimal(src, bm) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i].clone()));
                    extend_bitmap(
                        bm_out,
                        rows_before,
                        indices.iter().map(|&i| bitmap_is_valid(bm, i)),
                    );
                }
                TypedColumn::Decimal(src) => {
                    let rows_before = buf.len();
                    buf.extend(indices.iter().map(|&i| src[i].clone()));
                    extend_bitmap(bm_out, rows_before, indices.iter().map(|_| true));
                }
                _ => *self = Self::degraded_append(self, col, indices),
            },
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
            TypedColumn::DateTime(v) => {
                BatchColumn::DateTime(indices.iter().map(|&i| v[i]).collect())
            }
            TypedColumn::Utf8(v) => {
                BatchColumn::Utf8(indices.iter().map(|&i| v[i].clone()).collect())
            }
            TypedColumn::Decimal(v) => {
                BatchColumn::Decimal(indices.iter().map(|&i| v[i].clone()).collect())
            }
            TypedColumn::NullableI64(v, bm) => BatchColumn::NullableI64(
                indices.iter().map(|&i| v[i]).collect(),
                bitmap_from_indices(bm, indices),
            ),
            TypedColumn::NullableF64(v, bm) => BatchColumn::NullableF64(
                indices.iter().map(|&i| v[i]).collect(),
                bitmap_from_indices(bm, indices),
            ),
            TypedColumn::NullableI32(v, bm) => BatchColumn::NullableI32(
                indices.iter().map(|&i| v[i]).collect(),
                bitmap_from_indices(bm, indices),
            ),
            TypedColumn::NullableBool(v, bm) => BatchColumn::NullableBool(
                indices.iter().map(|&i| v[i]).collect(),
                bitmap_from_indices(bm, indices),
            ),
            TypedColumn::NullableDate(v, bm) => BatchColumn::NullableDate(
                indices.iter().map(|&i| v[i]).collect(),
                bitmap_from_indices(bm, indices),
            ),
            TypedColumn::NullableDateTime(v, bm) => BatchColumn::NullableDateTime(
                indices.iter().map(|&i| v[i]).collect(),
                bitmap_from_indices(bm, indices),
            ),
            TypedColumn::NullableUtf8(v, bm) => BatchColumn::NullableUtf8(
                indices.iter().map(|&i| v[i].clone()).collect(),
                bitmap_from_indices(bm, indices),
            ),
            TypedColumn::NullableDecimal(v, bm) => BatchColumn::NullableDecimal(
                indices.iter().map(|&i| v[i].clone()).collect(),
                bitmap_from_indices(bm, indices),
            ),
            TypedColumn::Fallback(v) => {
                BatchColumn::Fallback(indices.iter().map(|&i| v[i].clone()).collect())
            }
        }
    }

    /// Upgrade a plain typed column to its `Nullable*` form (past rows all
    /// valid), used when a later chunk introduces NULLs into the column.
    fn to_nullable(current: &Self) -> Self {
        match current {
            BatchColumn::I64(v) => {
                BatchColumn::NullableI64(v.clone(), vec![!0u64; v.len().div_ceil(64)])
            }
            BatchColumn::F64(v) => {
                BatchColumn::NullableF64(v.clone(), vec![!0u64; v.len().div_ceil(64)])
            }
            BatchColumn::I32(v) => {
                BatchColumn::NullableI32(v.clone(), vec![!0u64; v.len().div_ceil(64)])
            }
            BatchColumn::Bool(v) => {
                BatchColumn::NullableBool(v.clone(), vec![!0u64; v.len().div_ceil(64)])
            }
            BatchColumn::Date(v) => {
                BatchColumn::NullableDate(v.clone(), vec![!0u64; v.len().div_ceil(64)])
            }
            BatchColumn::DateTime(v) => {
                BatchColumn::NullableDateTime(v.clone(), vec![!0u64; v.len().div_ceil(64)])
            }
            BatchColumn::Utf8(v) => {
                BatchColumn::NullableUtf8(v.clone(), vec![!0u64; v.len().div_ceil(64)])
            }
            BatchColumn::Decimal(v) => {
                BatchColumn::NullableDecimal(v.clone(), vec![!0u64; v.len().div_ceil(64)])
            }
            _ => current.clone(),
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
            BatchColumn::DateTime(buf) => {
                if let Value::DateTime(x) = value {
                    buf.push(x.to_micros());
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
            BatchColumn::Decimal(buf) => {
                if let Value::Decimal128(x) = value {
                    buf.push(x.clone());
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::NullableI64(buf, bm) => {
                if let Value::BigInt(x) = value {
                    buf.push(*x);
                    extend_bitmap(bm, buf.len() - 1, std::iter::once(true));
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::NullableF64(buf, bm) => {
                if let Value::Double(x) = value {
                    buf.push(*x);
                    extend_bitmap(bm, buf.len() - 1, std::iter::once(true));
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::NullableI32(buf, bm) => {
                if let Value::Int(x) = value {
                    buf.push(*x);
                    extend_bitmap(bm, buf.len() - 1, std::iter::once(true));
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::NullableBool(buf, bm) => {
                if let Value::Bool(x) = value {
                    buf.push(*x);
                    extend_bitmap(bm, buf.len() - 1, std::iter::once(true));
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::NullableDate(buf, bm) => {
                if let Value::Date(x) = value {
                    buf.push(x.to_days());
                    extend_bitmap(bm, buf.len() - 1, std::iter::once(true));
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::NullableDateTime(buf, bm) => {
                if let Value::DateTime(x) = value {
                    buf.push(x.to_micros());
                    extend_bitmap(bm, buf.len() - 1, std::iter::once(true));
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::NullableUtf8(buf, bm) => {
                if let Value::String(x) = value {
                    buf.push(Arc::from(x.as_str()));
                    extend_bitmap(bm, buf.len() - 1, std::iter::once(true));
                } else {
                    *self = BatchColumn::degraded_push(self, value);
                }
            }
            BatchColumn::NullableDecimal(buf, bm) => {
                if let Value::Decimal128(x) = value {
                    buf.push(x.clone());
                    extend_bitmap(bm, buf.len() - 1, std::iter::once(true));
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
            BatchColumn::DateTime(v) => v.truncate(len),
            BatchColumn::Utf8(v) => v.truncate(len),
            BatchColumn::Decimal(v) => v.truncate(len),
            BatchColumn::NullableI64(v, bm) => {
                v.truncate(len);
                bm.truncate(len.div_ceil(64));
            }
            BatchColumn::NullableF64(v, bm) => {
                v.truncate(len);
                bm.truncate(len.div_ceil(64));
            }
            BatchColumn::NullableI32(v, bm) => {
                v.truncate(len);
                bm.truncate(len.div_ceil(64));
            }
            BatchColumn::NullableBool(v, bm) => {
                v.truncate(len);
                bm.truncate(len.div_ceil(64));
            }
            BatchColumn::NullableDate(v, bm) => {
                v.truncate(len);
                bm.truncate(len.div_ceil(64));
            }
            BatchColumn::NullableDateTime(v, bm) => {
                v.truncate(len);
                bm.truncate(len.div_ceil(64));
            }
            BatchColumn::NullableUtf8(v, bm) => {
                v.truncate(len);
                bm.truncate(len.div_ceil(64));
            }
            BatchColumn::NullableDecimal(v, bm) => {
                v.truncate(len);
                bm.truncate(len.div_ceil(64));
            }
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
            BatchColumn::DateTime(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i]).collect();
            }
            BatchColumn::Utf8(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i].clone()).collect();
            }
            BatchColumn::Decimal(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i].clone()).collect();
            }
            BatchColumn::NullableI64(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = bitmap_from_indices(&old_bm, perm);
            }
            BatchColumn::NullableF64(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = bitmap_from_indices(&old_bm, perm);
            }
            BatchColumn::NullableI32(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = bitmap_from_indices(&old_bm, perm);
            }
            BatchColumn::NullableBool(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = bitmap_from_indices(&old_bm, perm);
            }
            BatchColumn::NullableDate(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = bitmap_from_indices(&old_bm, perm);
            }
            BatchColumn::NullableDateTime(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = bitmap_from_indices(&old_bm, perm);
            }
            BatchColumn::NullableUtf8(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i].clone()).collect();
                *bm = bitmap_from_indices(&old_bm, perm);
            }
            BatchColumn::NullableDecimal(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i].clone()).collect();
                *bm = bitmap_from_indices(&old_bm, perm);
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
    use crate::executor::streaming::chunk::schema::{ColumnInfo, Schema};

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
    fn test_datetime_and_decimal_columns() {
        use graphdb_core::value::date_time::DateTimeValue;
        use graphdb_core::value::decimal128::Decimal128Value;
        let dt = |day: u32| {
            Value::DateTime(DateTimeValue {
                year: 2024,
                month: 1,
                day,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            })
        };
        let mut batch = ColumnarBatch::new(2);
        let mut chunk = DataChunk::new(
            vec![
                vec![dt(3), Value::Decimal128(Decimal128Value::from_i64(30))],
                vec![dt(1), Value::Decimal128(Decimal128Value::from_i64(10))],
                vec![dt(2), Value::Decimal128(Decimal128Value::from_i64(20))],
            ],
            Arc::new(Schema::new(vec![
                ColumnInfo {
                    name: "dt".to_string(),
                    data_type: "datetime".to_string(),
                },
                ColumnInfo {
                    name: "dec".to_string(),
                    data_type: "decimal128".to_string(),
                },
            ])),
        );
        chunk.build_typed_columns(true);
        batch.append_chunk(&chunk);
        assert!(batch.column(0).is_typed());
        assert!(batch.column(1).is_typed());
        assert_eq!(
            batch.column(0).compare_at(0, 1),
            Ordering::Greater,
            "row 0 (day 3) > row 1 (day 1)"
        );
        assert_eq!(
            batch.column(1).compare_at(0, 2),
            Ordering::Greater,
            "30 > 20"
        );
        assert_eq!(
            batch.column(0).value_at(1),
            dt(1),
            "DateTime materializes from micros"
        );
        assert_eq!(
            batch.column(1).value_at(2),
            Value::Decimal128(Decimal128Value::from_i64(20))
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
