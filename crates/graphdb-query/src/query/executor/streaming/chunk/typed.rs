//! Raw typed columns and SIMD-friendly batch evaluation
//!
//! Typed columns store dense `Vec<i64>`/`Vec<f64>`/`Vec<i32>`/`Vec<bool>` so
//! batch evaluation can operate on scalars (auto-vectorizable) instead of
//! constructing one `Value` per row.

use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::value::date_time::{DateTimeValue, DateValue};
use crate::core::value::decimal128::Decimal128Value;
use crate::core::value::NullType;
use crate::core::Value;
use std::cmp::Ordering;
use std::sync::Arc;

/// Kind of a typed fixed-size scalar column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedKind {
    I64,
    F64,
    I32,
    Bool,
    /// Date stored as days since epoch (i64), reusing the numeric eval path.
    Date,
    /// DateTime stored as micros since epoch (i64), reusing the numeric eval
    /// path; matches `cmp_datetime` ordering for normalized values.
    DateTime,
    /// String column stored as `Vec<Arc<str>>`, avoiding per-row `Value` boxing.
    Utf8,
    /// Decimal128 column stored as `Vec<Decimal128Value>` (comparison via
    /// `Ord` with decimal semantics).
    Decimal,
}

/// Typed column representation for fixed-size scalar columns.
///
/// `I64`/`F64`/`I32`/`Bool`/`Date`/`DateTime`/`Utf8`/`Decimal` columns are
/// stored as dense raw `Vec`s so that batch evaluation operates on scalars
/// (auto-vectorizable) instead of constructing one `Value` per row. Columns
/// that contain NULLs keep the typed representation through the matching
/// `Nullable*` variants (raw values + validity bitmap); columns that mix
/// kinds or carry non-scalar values fall back to [`TypedColumn::Fallback`].
///
/// Bitmap encoding: bit `i` of `bitmap[i / 64]` marks row `i` valid (`1` =
/// valid value, `0` = NULL). Invalid rows keep a placeholder in the value
/// vector so element access stays index-aligned.
#[derive(Debug, Clone)]
pub enum TypedColumn {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
    /// Days since epoch per row (see [`DateValue::to_days`]).
    Date(Vec<i64>),
    /// Micros since epoch per row (see [`DateTimeValue::to_micros`]).
    DateTime(Vec<i64>),
    Utf8(Vec<Arc<str>>),
    /// Decimal128 per row (decimal semantics, `Ord`).
    Decimal(Vec<Decimal128Value>),
    /// I64 column with a validity bitmap (see the encoding note above).
    NullableI64(Vec<i64>, Vec<u64>),
    /// F64 column with a validity bitmap.
    NullableF64(Vec<f64>, Vec<u64>),
    /// I32 column with a validity bitmap.
    NullableI32(Vec<i32>, Vec<u64>),
    /// Bool column with a validity bitmap.
    NullableBool(Vec<bool>, Vec<u64>),
    /// Date column with a validity bitmap.
    NullableDate(Vec<i64>, Vec<u64>),
    /// DateTime column with a validity bitmap.
    NullableDateTime(Vec<i64>, Vec<u64>),
    /// Utf8 column with a validity bitmap.
    NullableUtf8(Vec<Arc<str>>, Vec<u64>),
    /// Decimal column with a validity bitmap.
    NullableDecimal(Vec<Decimal128Value>, Vec<u64>),
    Fallback(Vec<Value>),
}

/// Whether row `idx` is valid in `bitmap` (bit set = valid, bit clear = NULL).
#[inline]
pub(super) fn bitmap_is_valid(bitmap: &[u64], idx: usize) -> bool {
    bitmap[idx / 64] & (1u64 << (idx % 64)) != 0
}

/// Set (`valid == true`) or clear the validity bit of row `idx`.
#[inline]
pub(super) fn bitmap_set_bit(bitmap: &mut [u64], idx: usize, valid: bool) {
    if valid {
        bitmap[idx / 64] |= 1u64 << (idx % 64);
    } else {
        bitmap[idx / 64] &= !(1u64 << (idx % 64));
    }
}

/// Gather the validity bits at `indices` into a new bitmap.
fn gather_bitmap(bitmap: &[u64], indices: &[usize]) -> Vec<u64> {
    let mut out = vec![0u64; indices.len().div_ceil(64)];
    for (j, &i) in indices.iter().enumerate() {
        bitmap_set_bit(&mut out, j, bitmap_is_valid(bitmap, i));
    }
    out
}

impl TypedColumn {
    pub fn len(&self) -> usize {
        match self {
            TypedColumn::I64(v) => v.len(),
            TypedColumn::F64(v) => v.len(),
            TypedColumn::I32(v) => v.len(),
            TypedColumn::Bool(v) => v.len(),
            TypedColumn::Date(v) => v.len(),
            TypedColumn::DateTime(v) => v.len(),
            TypedColumn::Utf8(v) => v.len(),
            TypedColumn::Decimal(v) => v.len(),
            TypedColumn::NullableI64(v, _) => v.len(),
            TypedColumn::NullableF64(v, _) => v.len(),
            TypedColumn::NullableI32(v, _) => v.len(),
            TypedColumn::NullableBool(v, _) => v.len(),
            TypedColumn::NullableDate(v, _) => v.len(),
            TypedColumn::NullableDateTime(v, _) => v.len(),
            TypedColumn::NullableUtf8(v, _) => v.len(),
            TypedColumn::NullableDecimal(v, _) => v.len(),
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

    /// Materialize the value at `idx` (O(1) for typed variants; NULL for
    /// invalid rows of the `Nullable*` variants).
    pub fn value_at(&self, idx: usize) -> Option<Value> {
        let null = || Some(Value::Null(NullType::Null));
        match self {
            TypedColumn::I64(v) => v.get(idx).map(|&x| Value::BigInt(x)),
            TypedColumn::F64(v) => v.get(idx).map(|&x| Value::Double(x)),
            TypedColumn::I32(v) => v.get(idx).map(|&x| Value::Int(x)),
            TypedColumn::Bool(v) => v.get(idx).map(|&x| Value::Bool(x)),
            TypedColumn::Date(v) => v.get(idx).map(|&x| Value::Date(DateValue::from_days(x))),
            TypedColumn::DateTime(v) => v
                .get(idx)
                .map(|&x| Value::DateTime(DateTimeValue::from_micros(x))),
            TypedColumn::Utf8(v) => v.get(idx).map(|x| Value::String(x.as_ref().into())),
            TypedColumn::Decimal(v) => v.get(idx).cloned().map(Value::Decimal128),
            TypedColumn::NullableI64(v, b) => {
                if bitmap_is_valid(b, idx) {
                    v.get(idx).map(|&x| Value::BigInt(x))
                } else {
                    null()
                }
            }
            TypedColumn::NullableF64(v, b) => {
                if bitmap_is_valid(b, idx) {
                    v.get(idx).map(|&x| Value::Double(x))
                } else {
                    null()
                }
            }
            TypedColumn::NullableI32(v, b) => {
                if bitmap_is_valid(b, idx) {
                    v.get(idx).map(|&x| Value::Int(x))
                } else {
                    null()
                }
            }
            TypedColumn::NullableBool(v, b) => {
                if bitmap_is_valid(b, idx) {
                    v.get(idx).map(|&x| Value::Bool(x))
                } else {
                    null()
                }
            }
            TypedColumn::NullableDate(v, b) => {
                if bitmap_is_valid(b, idx) {
                    v.get(idx).map(|&x| Value::Date(DateValue::from_days(x)))
                } else {
                    null()
                }
            }
            TypedColumn::NullableDateTime(v, b) => {
                if bitmap_is_valid(b, idx) {
                    v.get(idx)
                        .map(|&x| Value::DateTime(DateTimeValue::from_micros(x)))
                } else {
                    null()
                }
            }
            TypedColumn::NullableUtf8(v, b) => {
                if bitmap_is_valid(b, idx) {
                    v.get(idx).map(|x| Value::String(x.as_ref().into()))
                } else {
                    null()
                }
            }
            TypedColumn::NullableDecimal(v, b) => {
                if bitmap_is_valid(b, idx) {
                    v.get(idx).cloned().map(Value::Decimal128)
                } else {
                    null()
                }
            }
            TypedColumn::Fallback(v) => v.get(idx).cloned(),
        }
    }

    /// Convert the whole column into `Vec<Value>`.
    pub fn to_values(&self) -> Vec<Value> {
        match self {
            TypedColumn::I64(v) => v.iter().map(|&x| Value::BigInt(x)).collect(),
            TypedColumn::F64(v) => v.iter().map(|&x| Value::Double(x)).collect(),
            TypedColumn::I32(v) => v.iter().map(|&x| Value::Int(x)).collect(),
            TypedColumn::Bool(v) => v.iter().map(|&x| Value::Bool(x)).collect(),
            TypedColumn::Date(v) => v
                .iter()
                .map(|&x| Value::Date(DateValue::from_days(x)))
                .collect(),
            TypedColumn::DateTime(v) => v
                .iter()
                .map(|&x| Value::DateTime(DateTimeValue::from_micros(x)))
                .collect(),
            TypedColumn::Utf8(v) => v.iter().map(|x| Value::String(x.as_ref().into())).collect(),
            TypedColumn::Decimal(v) => v.iter().map(|x| Value::Decimal128(x.clone())).collect(),
            TypedColumn::NullableI64(v, b) => v
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    if bitmap_is_valid(b, i) {
                        Value::BigInt(x)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedColumn::NullableF64(v, b) => v
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    if bitmap_is_valid(b, i) {
                        Value::Double(x)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedColumn::NullableI32(v, b) => v
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    if bitmap_is_valid(b, i) {
                        Value::Int(x)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedColumn::NullableBool(v, b) => v
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    if bitmap_is_valid(b, i) {
                        Value::Bool(x)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedColumn::NullableDate(v, b) => v
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    if bitmap_is_valid(b, i) {
                        Value::Date(DateValue::from_days(x))
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedColumn::NullableDateTime(v, b) => v
                .iter()
                .enumerate()
                .map(|(i, &x)| {
                    if bitmap_is_valid(b, i) {
                        Value::DateTime(DateTimeValue::from_micros(x))
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedColumn::NullableUtf8(v, b) => v
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    if bitmap_is_valid(b, i) {
                        Value::String(x.as_ref().into())
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedColumn::NullableDecimal(v, b) => v
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    if bitmap_is_valid(b, i) {
                        Value::Decimal128(x.clone())
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedColumn::Fallback(v) => v.clone(),
        }
    }

    /// Estimated heap bytes of this column (for memory accounting).
    pub fn estimated_size(&self) -> usize {
        match self {
            TypedColumn::I64(v) => v.capacity() * std::mem::size_of::<i64>(),
            TypedColumn::F64(v) => v.capacity() * std::mem::size_of::<f64>(),
            TypedColumn::I32(v) => v.capacity() * std::mem::size_of::<i32>(),
            TypedColumn::Bool(v) => v.capacity() * std::mem::size_of::<bool>(),
            TypedColumn::Date(v) => v.capacity() * std::mem::size_of::<i64>(),
            TypedColumn::DateTime(v) => v.capacity() * std::mem::size_of::<i64>(),
            TypedColumn::Utf8(v) => v.iter().map(|s| s.len()).sum(),
            TypedColumn::Decimal(v) => v.capacity() * std::mem::size_of::<Decimal128Value>(),
            TypedColumn::NullableI64(v, b) => {
                v.capacity() * std::mem::size_of::<i64>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            TypedColumn::NullableF64(v, b) => {
                v.capacity() * std::mem::size_of::<f64>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            TypedColumn::NullableI32(v, b) => {
                v.capacity() * std::mem::size_of::<i32>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            TypedColumn::NullableBool(v, b) => {
                v.capacity() * std::mem::size_of::<bool>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            TypedColumn::NullableDate(v, b) => {
                v.capacity() * std::mem::size_of::<i64>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            TypedColumn::NullableDateTime(v, b) => {
                v.capacity() * std::mem::size_of::<i64>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            TypedColumn::NullableUtf8(v, b) => {
                v.iter().map(|s| s.len()).sum::<usize>() + b.capacity() * std::mem::size_of::<u64>()
            }
            TypedColumn::NullableDecimal(v, b) => {
                v.capacity() * std::mem::size_of::<Decimal128Value>()
                    + b.capacity() * std::mem::size_of::<u64>()
            }
            TypedColumn::Fallback(v) => v.iter().map(Value::estimated_size).sum(),
        }
    }
}

// ── TypedBatch: internal representation for typed evaluation ──

/// A batch of raw typed values produced by the typed evaluator.
///
/// Mirrors `Value::BigInt`/`Value::Double`/`Value::Int`/`Value::Bool`/
/// `Value::Date`/`Value::DateTime`/`Value::String`/`Value::Decimal128` in
/// raw space; converted to `Vec<Value>` once at the end of evaluation. The
/// `Nullable*` variants carry a validity bitmap (`1` = valid, `0` = NULL)
/// and materialize NULL for invalid rows.
#[derive(Debug, Clone)]
pub(super) enum TypedBatch {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
    /// Days since epoch per row (see [`DateValue::to_days`]).
    Date(Vec<i64>),
    /// Micros since epoch per row (see [`DateTimeValue::to_micros`]).
    DateTime(Vec<i64>),
    Utf8(Vec<Arc<str>>),
    /// Decimal128 per row (decimal semantics, `Ord`).
    Decimal(Vec<Decimal128Value>),
    /// I64 batch with a validity bitmap.
    NullableI64(Vec<i64>, Vec<u64>),
    /// F64 batch with a validity bitmap.
    NullableF64(Vec<f64>, Vec<u64>),
    /// I32 batch with a validity bitmap.
    NullableI32(Vec<i32>, Vec<u64>),
    /// Bool batch with a validity bitmap.
    NullableBool(Vec<bool>, Vec<u64>),
    /// Date batch with a validity bitmap.
    NullableDate(Vec<i64>, Vec<u64>),
    /// DateTime batch with a validity bitmap.
    NullableDateTime(Vec<i64>, Vec<u64>),
    /// Utf8 batch with a validity bitmap.
    NullableUtf8(Vec<Arc<str>>, Vec<u64>),
    /// Decimal batch with a validity bitmap.
    NullableDecimal(Vec<Decimal128Value>, Vec<u64>),
}

impl TypedBatch {
    pub(super) fn into_values(self) -> Vec<Value> {
        match self {
            TypedBatch::I64(v) => v.into_iter().map(Value::BigInt).collect(),
            TypedBatch::F64(v) => v.into_iter().map(Value::Double).collect(),
            TypedBatch::I32(v) => v.into_iter().map(Value::Int).collect(),
            TypedBatch::Bool(v) => v.into_iter().map(Value::Bool).collect(),
            TypedBatch::Date(v) => v
                .into_iter()
                .map(|d| Value::Date(DateValue::from_days(d)))
                .collect(),
            TypedBatch::DateTime(v) => v
                .into_iter()
                .map(|d| Value::DateTime(DateTimeValue::from_micros(d)))
                .collect(),
            TypedBatch::Utf8(v) => v
                .into_iter()
                .map(|s| Value::String(s.as_ref().into()))
                .collect(),
            TypedBatch::Decimal(v) => v.into_iter().map(Value::Decimal128).collect(),
            TypedBatch::NullableI64(v, b) => v
                .into_iter()
                .enumerate()
                .map(|(i, x)| {
                    if bitmap_is_valid(&b, i) {
                        Value::BigInt(x)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedBatch::NullableF64(v, b) => v
                .into_iter()
                .enumerate()
                .map(|(i, x)| {
                    if bitmap_is_valid(&b, i) {
                        Value::Double(x)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedBatch::NullableI32(v, b) => v
                .into_iter()
                .enumerate()
                .map(|(i, x)| {
                    if bitmap_is_valid(&b, i) {
                        Value::Int(x)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedBatch::NullableBool(v, b) => v
                .into_iter()
                .enumerate()
                .map(|(i, x)| {
                    if bitmap_is_valid(&b, i) {
                        Value::Bool(x)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedBatch::NullableDate(v, b) => v
                .into_iter()
                .enumerate()
                .map(|(i, d)| {
                    if bitmap_is_valid(&b, i) {
                        Value::Date(DateValue::from_days(d))
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedBatch::NullableDateTime(v, b) => v
                .into_iter()
                .enumerate()
                .map(|(i, d)| {
                    if bitmap_is_valid(&b, i) {
                        Value::DateTime(DateTimeValue::from_micros(d))
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedBatch::NullableUtf8(v, b) => v
                .into_iter()
                .enumerate()
                .map(|(i, s)| {
                    if bitmap_is_valid(&b, i) {
                        Value::String(s.as_ref().into())
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
            TypedBatch::NullableDecimal(v, b) => v
                .into_iter()
                .enumerate()
                .map(|(i, d)| {
                    if bitmap_is_valid(&b, i) {
                        Value::Decimal128(d)
                    } else {
                        Value::Null(NullType::Null)
                    }
                })
                .collect(),
        }
    }
}

// ── Typed batch evaluation functions ──

/// Borrow a typed column as a raw batch (`Fallback` columns are not typed).
pub(super) fn typed_column_batch(column: &TypedColumn) -> Option<TypedBatch> {
    match column {
        TypedColumn::I64(v) => Some(TypedBatch::I64(v.clone())),
        TypedColumn::F64(v) => Some(TypedBatch::F64(v.clone())),
        TypedColumn::I32(v) => Some(TypedBatch::I32(v.clone())),
        TypedColumn::Bool(v) => Some(TypedBatch::Bool(v.clone())),
        TypedColumn::Date(v) => Some(TypedBatch::Date(v.clone())),
        TypedColumn::DateTime(v) => Some(TypedBatch::DateTime(v.clone())),
        TypedColumn::Utf8(v) => Some(TypedBatch::Utf8(v.clone())),
        TypedColumn::Decimal(v) => Some(TypedBatch::Decimal(v.clone())),
        TypedColumn::NullableI64(v, b) => Some(TypedBatch::NullableI64(v.clone(), b.clone())),
        TypedColumn::NullableF64(v, b) => Some(TypedBatch::NullableF64(v.clone(), b.clone())),
        TypedColumn::NullableI32(v, b) => Some(TypedBatch::NullableI32(v.clone(), b.clone())),
        TypedColumn::NullableBool(v, b) => Some(TypedBatch::NullableBool(v.clone(), b.clone())),
        TypedColumn::NullableDate(v, b) => Some(TypedBatch::NullableDate(v.clone(), b.clone())),
        TypedColumn::NullableDateTime(v, b) => {
            Some(TypedBatch::NullableDateTime(v.clone(), b.clone()))
        }
        TypedColumn::NullableUtf8(v, b) => Some(TypedBatch::NullableUtf8(v.clone(), b.clone())),
        TypedColumn::NullableDecimal(v, b) => {
            Some(TypedBatch::NullableDecimal(v.clone(), b.clone()))
        }
        TypedColumn::Fallback(_) => None,
    }
}

/// Unary operators on raw typed batches.
///
/// Mirrors `UnaryOperationEvaluator` for the supported subset; anything else
/// returns `None` so the caller falls back to the value path. NULL-aware:
/// `+` is the identity (NULL stays NULL); `-` and `NOT` error on NULL in
/// the value path, so `Nullable*` operands fall back (`None`).
pub(super) fn typed_unary_batch(op: &UnaryOperator, batch: TypedBatch) -> Option<TypedBatch> {
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
            TypedBatch::Date(_)
            | TypedBatch::DateTime(_)
            | TypedBatch::Utf8(_)
            | TypedBatch::Decimal(_) => None,
            // NULL negation errors in the value path; fall back.
            TypedBatch::NullableI64(..)
            | TypedBatch::NullableF64(..)
            | TypedBatch::NullableI32(..)
            | TypedBatch::NullableBool(..)
            | TypedBatch::NullableDate(..)
            | TypedBatch::NullableDateTime(..)
            | TypedBatch::NullableUtf8(..)
            | TypedBatch::NullableDecimal(..) => None,
        },
        UnaryOperator::Not => match batch {
            TypedBatch::Bool(v) => Some(TypedBatch::Bool(v.into_iter().map(|b| !b).collect())),
            // NULL NOT errors in the value path; fall back.
            TypedBatch::NullableBool(..) => None,
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
/// value path, which handles cross-type coercion exactly. NULL-aware:
///
/// - comparisons: NULL rows compare by `Value` type priority (a NULL sorts
///   below every typed kind; NULL == NULL is true), so the result is always
///   a plain `Bool` batch;
/// - arithmetic and boolean And/Or on NULL rows error in the value path, so
///   nullable operands fall back (`None`).
pub(super) fn typed_binary_batch(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    use BinaryOperator::*;
    match op {
        Equal | NotEqual | LessThan | LessThanOrEqual | GreaterThan | GreaterThanOrEqual => {
            if is_nullable_batch(left) || is_nullable_batch(right) {
                nullable_compare_batches(op, left, right)
            } else {
                compare_typed_batches(op, left, right)
            }
        }
        Add | Subtract | Multiply | And | Or => {
            if is_nullable_batch(left) || is_nullable_batch(right) {
                // NULL operands make the value path error (arithmetic on
                // non-numeric types / And-Or on non-bools); fall back so the
                // error semantics stay exact.
                None
            } else {
                compare_or_arith_or_bool(op, left, right)
            }
        }
        _ => None,
    }
}

/// Non-nullable binary operators: arithmetic, boolean And/Or.
fn compare_or_arith_or_bool(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    use BinaryOperator::*;
    match op {
        Add | Subtract | Multiply => arith_typed_batches(op, left, right),
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
                Some(TypedBatch::Bool(vals))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Whether the batch carries a validity bitmap.
fn is_nullable_batch(batch: &TypedBatch) -> bool {
    matches!(
        batch,
        TypedBatch::NullableI64(..)
            | TypedBatch::NullableF64(..)
            | TypedBatch::NullableI32(..)
            | TypedBatch::NullableBool(..)
            | TypedBatch::NullableDate(..)
            | TypedBatch::NullableDateTime(..)
            | TypedBatch::NullableUtf8(..)
            | TypedBatch::NullableDecimal(..)
    )
}

/// Validity of row `idx`; `None` bitmap means all rows are valid.
#[inline]
fn valid_at(bitmap: Option<&[u64]>, idx: usize) -> bool {
    match bitmap {
        None => true,
        Some(b) => bitmap_is_valid(b, idx),
    }
}

/// Elementwise comparison producing `Vec<bool>`.
///
/// NULL rows compare by `Value` type priority: a NULL is smaller than every
/// typed kind (`Less` when the left operand is NULL, `Greater` when the
/// right operand is NULL, `Equal` when both are NULL), mirroring
/// `cmp_by_type_priority` for the `Value::Null(NullType::Null)` rows that
/// `build_typed_columns` admits into `Nullable*` columns.
fn zip_compare<T>(
    op: &BinaryOperator,
    l: &[T],
    r: &[T],
    lb: Option<&[u64]>,
    rb: Option<&[u64]>,
    cmp: fn(&T, &T) -> Ordering,
) -> Vec<bool> {
    use BinaryOperator::*;
    let mut out = Vec::with_capacity(l.len());
    for i in 0..l.len() {
        let o = match (valid_at(lb, i), valid_at(rb, i)) {
            (true, true) => cmp(&l[i], &r[i]),
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            (false, false) => Ordering::Equal,
        };
        let v = match op {
            Equal => o == Ordering::Equal,
            NotEqual => o != Ordering::Equal,
            LessThan => o == Ordering::Less,
            LessThanOrEqual => o != Ordering::Greater,
            GreaterThan => o == Ordering::Greater,
            GreaterThanOrEqual => o != Ordering::Less,
            _ => return out,
        };
        out.push(v);
    }
    out
}

/// Comparison operators with NULL-aware rows (at least one nullable
/// operand). Returns a plain `Bool` batch mirroring the value path.
fn nullable_compare_batches(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    use BinaryOperator::{Equal, NotEqual};
    // Integer family (i64/i32 and their nullable forms) promotes to i64.
    if let (Some((l, lb)), Some((r, rb))) = (numeric_i64_view(left), numeric_i64_view(right)) {
        let vals = zip_compare(
            op,
            &l,
            &r,
            lb.as_deref(),
            rb.as_deref(),
            |a: &i64, b: &i64| a.cmp(b),
        );
        return Some(TypedBatch::Bool(vals));
    }
    // Same-kind doubles use the NaN-aware ordering.
    if let (Some((l, lb)), Some((r, rb))) = (f64_view(left), f64_view(right)) {
        let vals = zip_compare(
            op,
            &l,
            &r,
            lb.as_deref(),
            rb.as_deref(),
            |a: &f64, b: &f64| cmp_f64_value(*a, *b),
        );
        return Some(TypedBatch::Bool(vals));
    }
    // Integer vs double promotes to f64 with `partial_cmp` semantics
    // (mirrors the cross-kind `Value` ordering).
    if let (Some((l, lb)), Some((r, rb))) = (numeric_f64_view(left), numeric_f64_view(right)) {
        let vals = zip_compare(
            op,
            &l,
            &r,
            lb.as_deref(),
            rb.as_deref(),
            |a: &f64, b: &f64| a.partial_cmp(b).unwrap_or(Ordering::Equal),
        );
        return Some(TypedBatch::Bool(vals));
    }
    // Strings compare lexicographically (bytewise).
    if let (Some((l, lb)), Some((r, rb))) = (utf8_view(left), utf8_view(right)) {
        let vals = zip_compare(
            op,
            &l,
            &r,
            lb.as_deref(),
            rb.as_deref(),
            |a: &Arc<str>, b: &Arc<str>| a.as_ref().cmp(b.as_ref()),
        );
        return Some(TypedBatch::Bool(vals));
    }
    // Date-times compare by micros-since-epoch, identical to `cmp_datetime`
    // for normalized values (see `chunk/kind.rs` for the limitation).
    if let (Some((l, lb)), Some((r, rb))) = (datetime_view(left), datetime_view(right)) {
        let vals = zip_compare(
            op,
            &l,
            &r,
            lb.as_deref(),
            rb.as_deref(),
            |a: &i64, b: &i64| a.cmp(b),
        );
        return Some(TypedBatch::Bool(vals));
    }
    // Decimals compare with decimal semantics (`Decimal128Value: Ord`),
    // mirroring the `Value::Decimal128` ordering.
    if let (Some((l, lb)), Some((r, rb))) = (decimal_view(left), decimal_view(right)) {
        let vals = zip_compare(
            op,
            &l,
            &r,
            lb.as_deref(),
            rb.as_deref(),
            |a: &Decimal128Value, b: &Decimal128Value| a.cmp(b),
        );
        return Some(TypedBatch::Bool(vals));
    }
    // Booleans support equality only.
    if matches!(op, Equal | NotEqual) {
        if let (Some((l, lb)), Some((r, rb))) = (bool_view(left), bool_view(right)) {
            let vals = zip_compare(
                op,
                &l,
                &r,
                lb.as_deref(),
                rb.as_deref(),
                |a: &bool, b: &bool| a.cmp(b),
            );
            return Some(TypedBatch::Bool(vals));
        }
    }
    None
}

/// View an integer batch as `Vec<i64>` (allocation-free for I64, promoted
/// for I32) plus its validity bitmap (`None` = all valid).
fn numeric_i64_view(batch: &TypedBatch) -> Option<(Vec<i64>, Option<Vec<u64>>)> {
    match batch {
        TypedBatch::I64(v) => Some((v.clone(), None)),
        TypedBatch::NullableI64(v, b) => Some((v.clone(), Some(b.clone()))),
        TypedBatch::I32(v) => Some((v.iter().map(|&x| i64::from(x)).collect(), None)),
        TypedBatch::NullableI32(v, b) => {
            Some((v.iter().map(|&x| i64::from(x)).collect(), Some(b.clone())))
        }
        _ => None,
    }
}

/// View an f64 batch (same-kind doubles only) plus its validity bitmap.
fn f64_view(batch: &TypedBatch) -> Option<(Vec<f64>, Option<Vec<u64>>)> {
    match batch {
        TypedBatch::F64(v) => Some((v.clone(), None)),
        TypedBatch::NullableF64(v, b) => Some((v.clone(), Some(b.clone()))),
        _ => None,
    }
}

/// View a numeric batch as `Vec<f64>` (for int-vs-double promotion) plus
/// its validity bitmap.
fn numeric_f64_view(batch: &TypedBatch) -> Option<(Vec<f64>, Option<Vec<u64>>)> {
    match batch {
        TypedBatch::F64(v) => Some((v.clone(), None)),
        TypedBatch::NullableF64(v, b) => Some((v.clone(), Some(b.clone()))),
        TypedBatch::I64(v) => Some((v.iter().map(|&x| x as f64).collect(), None)),
        TypedBatch::NullableI64(v, b) => {
            Some((v.iter().map(|&x| x as f64).collect(), Some(b.clone())))
        }
        TypedBatch::I32(v) => Some((v.iter().map(|&x| x as f64).collect(), None)),
        TypedBatch::NullableI32(v, b) => {
            Some((v.iter().map(|&x| x as f64).collect(), Some(b.clone())))
        }
        _ => None,
    }
}

/// String batch view: values plus optional validity bitmap (`None` = all valid).
type Utf8View = (Vec<Arc<str>>, Option<Vec<u64>>);

/// View a string batch plus its validity bitmap.
fn utf8_view(batch: &TypedBatch) -> Option<Utf8View> {
    match batch {
        TypedBatch::Utf8(v) => Some((v.clone(), None)),
        TypedBatch::NullableUtf8(v, b) => Some((v.clone(), Some(b.clone()))),
        _ => None,
    }
}

/// View a date-time batch as micros-since-epoch plus its validity bitmap.
fn datetime_view(batch: &TypedBatch) -> Option<(Vec<i64>, Option<Vec<u64>>)> {
    match batch {
        TypedBatch::DateTime(v) => Some((v.clone(), None)),
        TypedBatch::NullableDateTime(v, b) => Some((v.clone(), Some(b.clone()))),
        _ => None,
    }
}

/// View a decimal batch plus its validity bitmap.
fn decimal_view(batch: &TypedBatch) -> Option<(Vec<Decimal128Value>, Option<Vec<u64>>)> {
    match batch {
        TypedBatch::Decimal(v) => Some((v.clone(), None)),
        TypedBatch::NullableDecimal(v, b) => Some((v.clone(), Some(b.clone()))),
        _ => None,
    }
}

/// View a bool batch plus its validity bitmap.
fn bool_view(batch: &TypedBatch) -> Option<(Vec<bool>, Option<Vec<u64>>)> {
    match batch {
        TypedBatch::Bool(v) => Some((v.clone(), None)),
        TypedBatch::NullableBool(v, b) => Some((v.clone(), Some(b.clone()))),
        _ => None,
    }
}

/// Comparison operators on same-kind raw batches.
///
/// Same-kind paths are handled first (including the NaN-aware `cmp_f64`
/// ordering for doubles); then mixed integer kinds promote to i64 and
/// integer-vs-double promotes to f64, mirroring the `Value` cross-kind
/// semantics exactly.
fn compare_typed_batches(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    use BinaryOperator::*;
    if let Some(result) = match (left, right) {
        (TypedBatch::I64(l), TypedBatch::I64(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
            LessThan => l.iter().zip(r).map(|(&a, &b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a <= b).collect(),
            GreaterThan => l.iter().zip(r).map(|(&a, &b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a >= b).collect(),
            _ => return None,
        })),
        (TypedBatch::F64(l), TypedBatch::F64(r)) => Some(TypedBatch::Bool(match op {
            Equal => l
                .iter()
                .zip(r)
                .map(|(&a, &b)| cmp_f64_value(a, b) == Ordering::Equal)
                .collect(),
            NotEqual => l
                .iter()
                .zip(r)
                .map(|(&a, &b)| cmp_f64_value(a, b) != Ordering::Equal)
                .collect(),
            LessThan => l
                .iter()
                .zip(r)
                .map(|(&a, &b)| cmp_f64_value(a, b) == Ordering::Less)
                .collect(),
            LessThanOrEqual => l
                .iter()
                .zip(r)
                .map(|(&a, &b)| cmp_f64_value(a, b) != Ordering::Greater)
                .collect(),
            GreaterThan => l
                .iter()
                .zip(r)
                .map(|(&a, &b)| cmp_f64_value(a, b) == Ordering::Greater)
                .collect(),
            GreaterThanOrEqual => l
                .iter()
                .zip(r)
                .map(|(&a, &b)| cmp_f64_value(a, b) != Ordering::Less)
                .collect(),
            _ => return None,
        })),
        (TypedBatch::I32(l), TypedBatch::I32(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
            LessThan => l.iter().zip(r).map(|(&a, &b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a <= b).collect(),
            GreaterThan => l.iter().zip(r).map(|(&a, &b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a >= b).collect(),
            _ => return None,
        })),
        // Date values compare by days-since-epoch, which matches the
        // year/month/day ordering of `Value` exactly.
        (TypedBatch::Date(l), TypedBatch::Date(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
            LessThan => l.iter().zip(r).map(|(&a, &b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a <= b).collect(),
            GreaterThan => l.iter().zip(r).map(|(&a, &b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a >= b).collect(),
            _ => return None,
        })),
        // DateTime values compare by micros-since-epoch, which matches the
        // `cmp_datetime` field ordering for normalized values.
        (TypedBatch::DateTime(l), TypedBatch::DateTime(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
            LessThan => l.iter().zip(r).map(|(&a, &b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a <= b).collect(),
            GreaterThan => l.iter().zip(r).map(|(&a, &b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a >= b).collect(),
            _ => return None,
        })),
        // Decimal values compare with decimal semantics (`Ord`), mirroring
        // the `Value::Decimal128` ordering exactly.
        (TypedBatch::Decimal(l), TypedBatch::Decimal(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(a, b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(a, b)| a != b).collect(),
            LessThan => l.iter().zip(r).map(|(a, b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(r).map(|(a, b)| a <= b).collect(),
            GreaterThan => l.iter().zip(r).map(|(a, b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(r).map(|(a, b)| a >= b).collect(),
            _ => return None,
        })),
        // Strings compare lexicographically (bytewise), mirroring the
        // `Value::String` ordering used by the per-row path.
        (TypedBatch::Utf8(l), TypedBatch::Utf8(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(a, b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(a, b)| a != b).collect(),
            LessThan => l
                .iter()
                .zip(r)
                .map(|(a, b)| a.as_ref() < b.as_ref())
                .collect(),
            LessThanOrEqual => l
                .iter()
                .zip(r)
                .map(|(a, b)| a.as_ref() <= b.as_ref())
                .collect(),
            GreaterThan => l
                .iter()
                .zip(r)
                .map(|(a, b)| a.as_ref() > b.as_ref())
                .collect(),
            GreaterThanOrEqual => l
                .iter()
                .zip(r)
                .map(|(a, b)| a.as_ref() >= b.as_ref())
                .collect(),
            _ => return None,
        })),
        (TypedBatch::Bool(l), TypedBatch::Bool(r)) if matches!(op, Equal | NotEqual) => {
            Some(TypedBatch::Bool(match op {
                Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
                NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
                _ => return None,
            }))
        }
        _ => None,
    } {
        return Some(result);
    }

    // Mixed integer kinds promote to i64 (mirrors `Value` cross-type
    // integer comparison: promote to i64). The validity bitmaps are always
    // `None` here (the nullable path never reaches this function).
    if let (Some((l, _)), Some((r, _))) = (numeric_i64_view(left), numeric_i64_view(right)) {
        return Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(&r).map(|(&a, &b)| a == b).collect(),
            NotEqual => l.iter().zip(&r).map(|(&a, &b)| a != b).collect(),
            LessThan => l.iter().zip(&r).map(|(&a, &b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(&r).map(|(&a, &b)| a <= b).collect(),
            GreaterThan => l.iter().zip(&r).map(|(&a, &b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(&r).map(|(&a, &b)| a >= b).collect(),
            _ => return None,
        }));
    }

    // Integer vs double promotes to f64. `Value` uses plain `partial_cmp`
    // for cross-kind ordering (a NaN operand compares Equal to anything) and
    // exact `==` for equality — distinct from the same-kind NaN-aware
    // `cmp_f64`, so the cross-kind path must NOT reuse it.
    if let (Some(l), TypedBatch::F64(r)) = (int_as_f64(left), right) {
        return int_f64_compare(op, &l, r);
    }
    if let (TypedBatch::F64(l), Some(r)) = (left, int_as_f64(right)) {
        return int_f64_compare(op, l, &r);
    }
    None
}

/// View an integer batch as `Vec<f64>` (for int-vs-double promotion).
fn int_as_f64(batch: &TypedBatch) -> Option<Vec<f64>> {
    match batch {
        TypedBatch::I64(v) => Some(v.iter().map(|&x| x as f64).collect()),
        TypedBatch::I32(v) => Some(v.iter().map(|&x| x as f64).collect()),
        _ => None,
    }
}

/// Cross-kind integer-vs-double comparison mirroring `Value` semantics:
/// ordering via `partial_cmp().unwrap_or(Equal)`, equality via exact `==`.
/// Returns `None` for non-comparison operators.
fn int_f64_compare(op: &BinaryOperator, left: &[f64], right: &[f64]) -> Option<TypedBatch> {
    use BinaryOperator::*;
    Some(TypedBatch::Bool(match op {
        Equal => left.iter().zip(right).map(|(&a, &b)| a == b).collect(),
        NotEqual => left.iter().zip(right).map(|(&a, &b)| a != b).collect(),
        LessThan => left
            .iter()
            .zip(right)
            .map(|(&a, &b)| a.partial_cmp(&b).unwrap_or(Ordering::Equal) == Ordering::Less)
            .collect(),
        LessThanOrEqual => left
            .iter()
            .zip(right)
            .map(|(&a, &b)| a.partial_cmp(&b).unwrap_or(Ordering::Equal) != Ordering::Greater)
            .collect(),
        GreaterThan => left
            .iter()
            .zip(right)
            .map(|(&a, &b)| a.partial_cmp(&b).unwrap_or(Ordering::Equal) == Ordering::Greater)
            .collect(),
        GreaterThanOrEqual => left
            .iter()
            .zip(right)
            .map(|(&a, &b)| a.partial_cmp(&b).unwrap_or(Ordering::Equal) != Ordering::Less)
            .collect(),
        _ => return None,
    }))
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

/// Arithmetic operators on raw batches.
///
/// Same-kind paths are handled first (wrapping for ints); mixed integer
/// kinds promote to i64 and integer-vs-double promotes to f64, mirroring
/// the `Value` promotion rules.
fn arith_typed_batches(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    use BinaryOperator::{Add, Multiply, Subtract};
    if let Some(result) = match (left, right) {
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
    } {
        return Some(result);
    }

    // Mixed integer kinds promote to i64 (mirrors `Value` promotion, e.g.
    // Int + BigInt -> BigInt). The validity bitmaps are always `None` here
    // (the nullable path never reaches this function).
    if let (Some((l, _)), Some((r, _))) = (numeric_i64_view(left), numeric_i64_view(right)) {
        return Some(TypedBatch::I64(
            l.iter()
                .zip(&r)
                .map(|(&a, &b)| match op {
                    Add => a.wrapping_add(b),
                    Subtract => a.wrapping_sub(b),
                    Multiply => a.wrapping_mul(b),
                    _ => unreachable!("arith only"),
                })
                .collect(),
        ));
    }

    // Integer vs double promotes to f64 (mirrors `Value` promotion, e.g.
    // Int + Double -> Double). The validity bitmaps are always `None` here
    // (the nullable path never reaches this function).
    if let (Some((l, _)), Some((r, _))) = (numeric_f64_view(left), numeric_f64_view(right)) {
        return Some(TypedBatch::F64(
            l.iter()
                .zip(&r)
                .map(|(&a, &b)| match op {
                    Add => a + b,
                    Subtract => a - b,
                    Multiply => a * b,
                    _ => unreachable!("arith only"),
                })
                .collect(),
        ));
    }
    None
}

/// Type casts on raw typed batches.
///
/// Mirrors `ExpressionEvaluator::eval_type_cast` for numeric targets. Casts
/// that may produce NULL (e.g. non-finite f64 → int) are NOT served by the
/// typed path and fall back to the value path. `Nullable*` batches keep
/// their validity bitmap unchanged.
pub(super) fn typed_cast_batch(
    batch: TypedBatch,
    target_type: &crate::core::types::DataType,
) -> Option<TypedBatch> {
    use crate::core::types::DataType;
    match target_type {
        DataType::Int | DataType::BigInt => match batch {
            TypedBatch::I64(v) => Some(TypedBatch::I64(v)),
            TypedBatch::I32(v) => Some(TypedBatch::I64(v.into_iter().map(i64::from).collect())),
            TypedBatch::NullableI64(v, b) => Some(TypedBatch::NullableI64(v, b)),
            TypedBatch::NullableI32(v, b) => Some(TypedBatch::NullableI64(
                v.into_iter().map(i64::from).collect(),
                b,
            )),
            _ => None,
        },
        DataType::Double => match batch {
            TypedBatch::F64(v) => Some(TypedBatch::F64(v)),
            TypedBatch::I64(v) => Some(TypedBatch::F64(v.into_iter().map(|x| x as f64).collect())),
            TypedBatch::I32(v) => Some(TypedBatch::F64(v.into_iter().map(|x| x as f64).collect())),
            TypedBatch::NullableF64(v, b) => Some(TypedBatch::NullableF64(v, b)),
            TypedBatch::NullableI64(v, b) => Some(TypedBatch::NullableF64(
                v.into_iter().map(|x| x as f64).collect(),
                b,
            )),
            TypedBatch::NullableI32(v, b) => Some(TypedBatch::NullableF64(
                v.into_iter().map(|x| x as f64).collect(),
                b,
            )),
            _ => None,
        },
        DataType::Bool => match batch {
            TypedBatch::I64(v) => Some(TypedBatch::Bool(v.into_iter().map(|x| x != 0).collect())),
            TypedBatch::F64(v) => Some(TypedBatch::Bool(v.into_iter().map(|x| x != 0.0).collect())),
            TypedBatch::I32(v) => Some(TypedBatch::Bool(v.into_iter().map(|x| x != 0).collect())),
            TypedBatch::Bool(v) => Some(TypedBatch::Bool(v)),
            TypedBatch::NullableI64(v, b) => Some(TypedBatch::NullableBool(
                v.into_iter().map(|x| x != 0).collect(),
                b,
            )),
            TypedBatch::NullableF64(v, b) => Some(TypedBatch::NullableBool(
                v.into_iter().map(|x| x != 0.0).collect(),
                b,
            )),
            TypedBatch::NullableI32(v, b) => Some(TypedBatch::NullableBool(
                v.into_iter().map(|x| x != 0).collect(),
                b,
            )),
            TypedBatch::NullableBool(v, b) => Some(TypedBatch::NullableBool(v, b)),
            _ => None,
        },
        _ => None,
    }
}

/// Gather a typed column's entries at `indices`.
pub(super) fn gather_typed_column(column: &TypedColumn, indices: &[usize]) -> TypedColumn {
    match column {
        TypedColumn::I64(v) => TypedColumn::I64(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::F64(v) => TypedColumn::F64(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::I32(v) => TypedColumn::I32(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::Bool(v) => TypedColumn::Bool(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::Date(v) => TypedColumn::Date(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::DateTime(v) => TypedColumn::DateTime(indices.iter().map(|&i| v[i]).collect()),
        TypedColumn::Utf8(v) => TypedColumn::Utf8(indices.iter().map(|&i| v[i].clone()).collect()),
        TypedColumn::Decimal(v) => {
            TypedColumn::Decimal(indices.iter().map(|&i| v[i].clone()).collect())
        }
        TypedColumn::NullableI64(v, b) => TypedColumn::NullableI64(
            indices.iter().map(|&i| v[i]).collect(),
            gather_bitmap(b, indices),
        ),
        TypedColumn::NullableF64(v, b) => TypedColumn::NullableF64(
            indices.iter().map(|&i| v[i]).collect(),
            gather_bitmap(b, indices),
        ),
        TypedColumn::NullableI32(v, b) => TypedColumn::NullableI32(
            indices.iter().map(|&i| v[i]).collect(),
            gather_bitmap(b, indices),
        ),
        TypedColumn::NullableBool(v, b) => TypedColumn::NullableBool(
            indices.iter().map(|&i| v[i]).collect(),
            gather_bitmap(b, indices),
        ),
        TypedColumn::NullableDate(v, b) => TypedColumn::NullableDate(
            indices.iter().map(|&i| v[i]).collect(),
            gather_bitmap(b, indices),
        ),
        TypedColumn::NullableDateTime(v, b) => TypedColumn::NullableDateTime(
            indices.iter().map(|&i| v[i]).collect(),
            gather_bitmap(b, indices),
        ),
        TypedColumn::NullableUtf8(v, b) => TypedColumn::NullableUtf8(
            indices.iter().map(|&i| v[i].clone()).collect(),
            gather_bitmap(b, indices),
        ),
        TypedColumn::NullableDecimal(v, b) => TypedColumn::NullableDecimal(
            indices.iter().map(|&i| v[i].clone()).collect(),
            gather_bitmap(b, indices),
        ),
        TypedColumn::Fallback(v) => {
            TypedColumn::Fallback(indices.iter().map(|&i| v[i].clone()).collect())
        }
    }
}
