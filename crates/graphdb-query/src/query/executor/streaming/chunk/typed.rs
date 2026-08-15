//! Raw typed columns and SIMD-friendly batch evaluation
//!
//! Typed columns store dense `Vec<i64>`/`Vec<f64>`/`Vec<i32>`/`Vec<bool>` so
//! batch evaluation can operate on scalars (auto-vectorizable) instead of
//! constructing one `Value` per row.

use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::value::date_time::DateValue;
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
    /// String column stored as `Vec<Arc<str>>`, avoiding per-row `Value` boxing.
    Utf8,
}

/// Typed column representation for fixed-size scalar columns.
///
/// `I64`/`F64`/`I32`/`Bool`/`Date`/`Utf8` columns are stored as dense raw
/// `Vec`s so that batch evaluation operates on scalars (auto-vectorizable)
/// instead of constructing one `Value` per row. Columns that contain NULLs,
/// mixed types, or non-scalar values fall back to [`TypedColumn::Fallback`].
#[derive(Debug, Clone)]
pub enum TypedColumn {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
    /// Days since epoch per row (see [`DateValue::to_days`]).
    Date(Vec<i64>),
    Utf8(Vec<Arc<str>>),
    Fallback(Vec<Value>),
}

impl TypedColumn {
    pub fn len(&self) -> usize {
        match self {
            TypedColumn::I64(v) => v.len(),
            TypedColumn::F64(v) => v.len(),
            TypedColumn::I32(v) => v.len(),
            TypedColumn::Bool(v) => v.len(),
            TypedColumn::Date(v) => v.len(),
            TypedColumn::Utf8(v) => v.len(),
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
            TypedColumn::Bool(v) => v.get(idx).map(|&x| Value::Bool(x)),
            TypedColumn::Date(v) => v.get(idx).map(|&x| Value::Date(DateValue::from_days(x))),
            TypedColumn::Utf8(v) => v.get(idx).map(|x| Value::String(x.as_ref().into())),
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
            TypedColumn::Utf8(v) => v.iter().map(|x| Value::String(x.as_ref().into())).collect(),
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
            TypedColumn::Utf8(v) => v.iter().map(|s| s.len()).sum(),
            TypedColumn::Fallback(v) => v.iter().map(Value::estimated_size).sum(),
        }
    }
}

// ── TypedBatch: internal representation for typed evaluation ──

/// A batch of raw typed values produced by the typed evaluator.
///
/// Mirrors `Value::BigInt`/`Value::Double`/`Value::Int`/`Value::Bool`/
/// `Value::Date`/`Value::String` in raw space; converted to `Vec<Value>`
/// once at the end of evaluation.
#[derive(Debug, Clone)]
pub(super) enum TypedBatch {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
    /// Days since epoch per row (see [`DateValue::to_days`]).
    Date(Vec<i64>),
    Utf8(Vec<Arc<str>>),
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
            TypedBatch::Utf8(v) => v
                .into_iter()
                .map(|s| Value::String(s.as_ref().into()))
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
        TypedColumn::Utf8(v) => Some(TypedBatch::Utf8(v.clone())),
        TypedColumn::Fallback(_) => None,
    }
}

/// Replicate a literal into a raw batch of `n` rows, when the literal has a
/// typed scalar kind (BigInt/Double/Int/Bool/Date/String).
pub(super) fn typed_literal_batch(value: &Value, n: usize) -> Option<TypedBatch> {
    match value {
        Value::BigInt(v) => Some(TypedBatch::I64(vec![*v; n])),
        Value::Double(v) => Some(TypedBatch::F64(vec![*v; n])),
        Value::Int(v) => Some(TypedBatch::I32(vec![*v; n])),
        Value::Bool(v) => Some(TypedBatch::Bool(vec![*v; n])),
        Value::Date(v) => Some(TypedBatch::Date(vec![v.to_days(); n])),
        Value::String(v) => Some(TypedBatch::Utf8(vec![Arc::from(v.as_str()); n])),
        _ => None,
    }
}

/// Unary operators on raw typed batches.
///
/// Mirrors `UnaryOperationEvaluator` for the supported subset; anything else
/// returns `None` so the caller falls back to the value path.
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
            TypedBatch::Date(_) | TypedBatch::Utf8(_) => None,
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
pub(super) fn typed_binary_batch(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    use BinaryOperator::*;
    match op {
        Equal | NotEqual | LessThan | LessThanOrEqual | GreaterThan | GreaterThanOrEqual => {
            compare_typed_batches(op, left, right)
        }
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
    // integer comparison: promote to i64).
    if let (Some(l), Some(r)) = (numeric_i64_view(left), numeric_i64_view(right)) {
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

/// View an integer batch as `Vec<i64>` (allocation-free for I64, promoted
/// for I32).
fn numeric_i64_view(batch: &TypedBatch) -> Option<Vec<i64>> {
    match batch {
        TypedBatch::I64(v) => Some(v.clone()),
        TypedBatch::I32(v) => Some(v.iter().map(|&x| i64::from(x)).collect()),
        _ => None,
    }
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
    // Int + BigInt -> BigInt).
    if let (Some(l), Some(r)) = (numeric_i64_view(left), numeric_i64_view(right)) {
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
    // Int + Double -> Double).
    if let (Some(l), Some(r)) = (numeric_f64_view(left), numeric_f64_view(right)) {
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

/// View a numeric batch as `Vec<f64>` (for int-vs-double promotion).
fn numeric_f64_view(batch: &TypedBatch) -> Option<Vec<f64>> {
    match batch {
        TypedBatch::F64(v) => Some(v.clone()),
        TypedBatch::I64(v) => Some(v.iter().map(|&x| x as f64).collect()),
        TypedBatch::I32(v) => Some(v.iter().map(|&x| x as f64).collect()),
        _ => None,
    }
}

/// Type casts on raw typed batches.
///
/// Mirrors `ExpressionEvaluator::eval_type_cast` for numeric targets. Casts
/// that may produce NULL (e.g. non-finite f64 → int) are NOT served by the
/// typed path and fall back to the value path.
pub(super) fn typed_cast_batch(
    batch: TypedBatch,
    target_type: &crate::core::types::DataType,
) -> Option<TypedBatch> {
    use crate::core::types::DataType;
    match target_type {
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
        TypedColumn::Utf8(v) => TypedColumn::Utf8(indices.iter().map(|&i| v[i].clone()).collect()),
        TypedColumn::Fallback(v) => {
            TypedColumn::Fallback(indices.iter().map(|&i| v[i].clone()).collect())
        }
    }
}
