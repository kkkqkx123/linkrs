use std::cmp::Ordering;
use std::sync::Arc;

use crate::executor::streaming::chunk::columnar_common::{
    bitmap_is_valid, gather_bitmap, gather_column,
};
use crate::executor::streaming::chunk::typed::{TypedColumn, TypedKind};
use crate::executor::streaming::helpers::compare_values;
use linkrs_core::value::date_time::{DateTimeValue, DateValue};
use linkrs_core::value::NullType;
use linkrs_core::Value;

use super::BatchColumn;

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

impl BatchColumn {
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

    /// Append the entries of a chunk column at `indices`.
    ///
    /// When `self` is Empty the kind is taken from the chunk column (a
    /// fallback chunk column starts a fallback batch column). A kind
    /// mismatch degrades the accumulated column to [`BatchColumn::Fallback`].
    /// A NULL introduced by a `Nullable*` chunk column upgrades a plain
    /// typed column to its `Nullable*` form (past rows become valid).
    pub(super) fn append_typed(&mut self, col: &TypedColumn, indices: &[usize]) {
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
        gather_column!(BatchColumn, col, indices)
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

    pub(super) fn append_row_value(&mut self, value: &Value) {
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

    pub(super) fn truncate(&mut self, len: usize) {
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

    pub(super) fn permute(&mut self, perm: &[usize]) {
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
                *bm = gather_bitmap(&old_bm, perm);
            }
            BatchColumn::NullableF64(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = gather_bitmap(&old_bm, perm);
            }
            BatchColumn::NullableI32(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = gather_bitmap(&old_bm, perm);
            }
            BatchColumn::NullableBool(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = gather_bitmap(&old_bm, perm);
            }
            BatchColumn::NullableDate(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = gather_bitmap(&old_bm, perm);
            }
            BatchColumn::NullableDateTime(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i]).collect();
                *bm = gather_bitmap(&old_bm, perm);
            }
            BatchColumn::NullableUtf8(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i].clone()).collect();
                *bm = gather_bitmap(&old_bm, perm);
            }
            BatchColumn::NullableDecimal(v, bm) => {
                let (old_v, old_bm) = (std::mem::take(v), std::mem::take(bm));
                *v = perm.iter().map(|&i| old_v[i].clone()).collect();
                *bm = gather_bitmap(&old_bm, perm);
            }
            BatchColumn::Fallback(v) => {
                let old = std::mem::take(v);
                *v = perm.iter().map(|&i| old[i].clone()).collect();
            }
        }
    }
}
