//! Raw typed columns and SIMD-friendly batch evaluation
//!
//! Typed columns store dense `Vec<i64>`/`Vec<f64>`/`Vec<i32>`/`Vec<bool>` so
//! batch evaluation can operate on scalars (auto-vectorizable) instead of
//! constructing one `Value` per row.

use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::Value;
use std::cmp::Ordering;

/// Kind of a typed fixed-size scalar column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedKind {
    I64,
    F64,
    I32,
    Bool,
}

/// Typed column representation for fixed-size scalar columns.
///
/// `I64`/`F64`/`I32`/`Bool` columns are stored as dense raw `Vec`s so that batch
/// evaluation operates on scalars (auto-vectorizable) instead of constructing
/// one `Value` per row. Columns that contain NULLs, mixed types, or
/// non-scalar values fall back to [`TypedColumn::Fallback`].
#[derive(Debug, Clone)]
pub enum TypedColumn {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
    Fallback(Vec<Value>),
}

impl TypedColumn {
    pub fn len(&self) -> usize {
        match self {
            TypedColumn::I64(v) => v.len(),
            TypedColumn::F64(v) => v.len(),
            TypedColumn::I32(v) => v.len(),
            TypedColumn::Bool(v) => v.len(),
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
            TypedColumn::Fallback(v) => v.iter().map(Value::estimated_size).sum(),
        }
    }
}

// ── TypedBatch: internal representation for typed evaluation ──

/// A batch of raw typed values produced by the typed evaluator.
///
/// Mirrors `Value::BigInt`/`Value::Double`/`Value::Int`/`Value::Bool` in
/// raw space; converted to `Vec<Value>` once at the end of evaluation.
#[derive(Debug, Clone)]
pub(super) enum TypedBatch {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
}

impl TypedBatch {
    pub(super) fn into_values(self) -> Vec<Value> {
        match self {
            TypedBatch::I64(v) => v.into_iter().map(Value::BigInt).collect(),
            TypedBatch::F64(v) => v.into_iter().map(Value::Double).collect(),
            TypedBatch::I32(v) => v.into_iter().map(Value::Int).collect(),
            TypedBatch::Bool(v) => v.into_iter().map(Value::Bool).collect(),
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
        TypedColumn::Fallback(_) => None,
    }
}

/// Replicate a literal into a raw batch of `n` rows, when the literal has a
/// typed scalar kind (BigInt/Double/Int/Bool).
pub(super) fn typed_literal_batch(value: &Value, n: usize) -> Option<TypedBatch> {
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
fn compare_typed_batches(
    op: &BinaryOperator,
    left: &TypedBatch,
    right: &TypedBatch,
) -> Option<TypedBatch> {
    use BinaryOperator::*;
    let batch = match (left, right) {
        (TypedBatch::I64(l), TypedBatch::I64(r)) => {
            TypedBatch::Bool(match op {
                Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
                NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
                LessThan => l.iter().zip(r).map(|(&a, &b)| a < b).collect(),
                LessThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a <= b).collect(),
                GreaterThan => l.iter().zip(r).map(|(&a, &b)| a > b).collect(),
                GreaterThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a >= b).collect(),
                _ => return None,
            })
        }
        (TypedBatch::F64(l), TypedBatch::F64(r)) => {
            TypedBatch::Bool(match op {
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
            })
        }
        (TypedBatch::I32(l), TypedBatch::I32(r)) => {
            TypedBatch::Bool(match op {
                Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
                NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
                LessThan => l.iter().zip(r).map(|(&a, &b)| a < b).collect(),
                LessThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a <= b).collect(),
                GreaterThan => l.iter().zip(r).map(|(&a, &b)| a > b).collect(),
                GreaterThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a >= b).collect(),
                _ => return None,
            })
        }
        (TypedBatch::Bool(l), TypedBatch::Bool(r))
            if matches!(op, Equal | NotEqual) =>
        {
            TypedBatch::Bool(match op {
                Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
                NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
                _ => return None,
            })
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
        TypedColumn::Fallback(v) => {
            TypedColumn::Fallback(indices.iter().map(|&i| v[i].clone()).collect())
        }
    }
}