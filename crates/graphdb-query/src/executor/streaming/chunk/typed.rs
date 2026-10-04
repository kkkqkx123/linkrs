//! Raw typed columns and SIMD-friendly batch evaluation
//!
//! Typed columns store dense `Vec<i64>`/`Vec<f64>`/`Vec<i32>`/`Vec<bool>` so
//! batch evaluation can operate on scalars (auto-vectorizable) instead of
//! constructing one `Value` per row.
//!
//! NULLs are carried by the matching `Nullable*` variant (values plus a
//! validity bitmap) rather than by widening every element to `Option<T>`.
//! The standalone bitmap benchmark in
//! `docs/archive/benches/columnar-necessity-verification.md` favoured
//! `Option<T>` for a plain `Vec<i64>` scan, but here the bitmap is what
//! keeps a NULL-bearing column on the typed path at all: the alternative is
//! degrading the whole column to `Fallback`. Re-measure both encodings
//! before changing this trade-off.

use std::sync::Arc;

use graphdb_core::value::date_time::{DateTimeValue, DateValue};
use graphdb_core::value::decimal128::Decimal128Value;
use graphdb_core::value::NullType;
use graphdb_core::Value;

use super::columnar_common::{bitmap_is_valid, column_variants};

mod evaluation;
mod operations;

pub(crate) use evaluation::{gather_typed_column, repeat_typed_column};
pub(crate) use operations::{
    typed_binary_batch, typed_cast_batch, typed_column_batch, typed_unary_batch,
};

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

// Typed column representation for fixed-size scalar columns: dense raw
// `Vec`s for the typed variants (see `columnar_common` for the shared
// variant list), `Nullable*` variants carrying a validity bitmap, and
// `Fallback` for mixed-kind columns.
//
// Bitmap encoding: bit `i` of `bitmap[i / 64]` marks row `i` valid (`1` =
// valid value, `0` = NULL). Invalid rows keep a placeholder in the value
// vector so element access stays index-aligned.
//
// The variant set is declared once by `column_variants!` in
// `columnar_common` and shared with `BatchColumn`; `len`, `is_empty`, and
// `estimated_size` are generated from that list.
// Immutable per-chunk snapshot; see `columnar_common` for the shared list.
column_variants!(TypedColumn);

impl TypedColumn {
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
