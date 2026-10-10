use std::cmp::Ordering;
use std::sync::Arc;

use linkrs_core::types::operators::{BinaryOperator, UnaryOperator};
use linkrs_core::value::decimal128::Decimal128Value;

use super::super::columnar_common::bitmap_is_valid;
use super::{TypedBatch, TypedColumn};

/// Borrow a typed column as a raw batch (`Fallback` columns are not typed).
pub(crate) fn typed_column_batch(column: &TypedColumn) -> Option<TypedBatch> {
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
        // Identity columns are not scalar-evaluable: expression evaluation
        // materializes them through `value_at` / `to_values` instead.
        TypedColumn::VertexIdentity(_) | TypedColumn::EdgeHeader(_) => None,
        TypedColumn::Fallback(_) => None,
    }
}

/// Unary operators on raw typed batches.
///
/// Mirrors `UnaryOperationEvaluator` for the supported subset; anything else
/// returns `None` so the caller falls back to the value path. NULL-aware:
/// `+` is the identity (NULL stays NULL); `-` and `NOT` error on NULL in
/// the value path, so `Nullable*` operands fall back (`None`).
pub(crate) fn typed_unary_batch(op: &UnaryOperator, batch: TypedBatch) -> Option<TypedBatch> {
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
pub(crate) fn typed_binary_batch(
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
        (TypedBatch::Date(l), TypedBatch::Date(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
            LessThan => l.iter().zip(r).map(|(&a, &b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a <= b).collect(),
            GreaterThan => l.iter().zip(r).map(|(&a, &b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a >= b).collect(),
            _ => return None,
        })),
        (TypedBatch::DateTime(l), TypedBatch::DateTime(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(&a, &b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(&a, &b)| a != b).collect(),
            LessThan => l.iter().zip(r).map(|(&a, &b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a <= b).collect(),
            GreaterThan => l.iter().zip(r).map(|(&a, &b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(r).map(|(&a, &b)| a >= b).collect(),
            _ => return None,
        })),
        (TypedBatch::Decimal(l), TypedBatch::Decimal(r)) => Some(TypedBatch::Bool(match op {
            Equal => l.iter().zip(r).map(|(a, b)| a == b).collect(),
            NotEqual => l.iter().zip(r).map(|(a, b)| a != b).collect(),
            LessThan => l.iter().zip(r).map(|(a, b)| a < b).collect(),
            LessThanOrEqual => l.iter().zip(r).map(|(a, b)| a <= b).collect(),
            GreaterThan => l.iter().zip(r).map(|(a, b)| a > b).collect(),
            GreaterThanOrEqual => l.iter().zip(r).map(|(a, b)| a >= b).collect(),
            _ => return None,
        })),
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
pub(crate) fn typed_cast_batch(
    batch: TypedBatch,
    target_type: &linkrs_core::types::DataType,
) -> Option<TypedBatch> {
    use linkrs_core::types::DataType;
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
