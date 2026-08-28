//! `Value → TypedKind` inference and typed column construction helpers.
//!
//! The typed column fast path is driven by runtime `Value` variants, not by
//! `DataType`: columns are typically built from a `Value` sequence with no
//! schema information at hand. The representative value is the first row; a
//! leading NULL probes the first non-NULL value so NULL-leading homogeneous
//! columns stay on the typed path (all-NULL columns fall back). All inference
//! from a representative `Value` to a [`TypedKind`] lives in this module so
//! that adding a new kind touches one file for "inference + collection".
//!
//! # Known limitation (shared with the `Date` fast path)
//!
//! `TypedKind::Date` and `TypedKind::DateTime` store days/micros-since-epoch
//! `i64` and reuse the numeric ordering path, which matches the `Value`
//! field-wise ordering only for **normalized** values: non-normalized fields
//! (e.g. month = 13, day = 32) can diverge from
//! `value_compare::cmp_date`/`cmp_datetime`. DDL validation must keep date
//! values normalized before they enter the typed path.

use std::sync::Arc;

use graphdb_core::value::decimal128::Decimal128Value;
use graphdb_core::value::NullType;
use graphdb_core::Value;

use super::typed::{TypedBatch, TypedColumn, TypedKind};

/// Infer the typed column kind from a representative `Value`.
///
/// `None` for values without a typed fast path (containers, ...). When
/// `None`, the column falls back to the per-row `Value` semantics.
pub fn value_to_kind(value: &Value) -> Option<TypedKind> {
    match value {
        Value::BigInt(_) => Some(TypedKind::I64),
        Value::Double(_) => Some(TypedKind::F64),
        Value::Int(_) => Some(TypedKind::I32),
        Value::Bool(_) => Some(TypedKind::Bool),
        Value::Date(_) => Some(TypedKind::Date),
        Value::DateTime(_) => Some(TypedKind::DateTime),
        Value::String(_) => Some(TypedKind::Utf8),
        Value::Decimal128(_) => Some(TypedKind::Decimal),
        _ => None,
    }
}

/// First value in `values` that is not NULL, or `None` when all are NULL.
///
/// Used to probe the column kind when the leading row is NULL, so
/// NULL-leading homogeneous columns stay on the typed path.
pub fn first_non_null<'a>(values: impl IntoIterator<Item = &'a Value>) -> Option<&'a Value> {
    values.into_iter().find(|v| !matches!(v, Value::Null(_)))
}

/// Outcome of pushing one value into a [`TypedColumnBuilder`].
pub enum PushOutcome {
    /// A non-NULL value of the column kind was pushed.
    Value,
    /// A NULL slot was pushed (a placeholder); the caller tracks the
    /// validity bitmap.
    Null,
    /// Kind mismatch; the caller discards the builder and falls back to
    /// [`TypedColumn::Fallback`].
    Mismatch,
}

/// Column-major accumulation buffer for one typed column during chunk build.
///
/// Owns the typed value buffer for a [`TypedKind`] so the row-scan skeleton
/// (capacity + per-row push + validity bitmap) is written once instead of
/// once per kind. On a kind mismatch the caller discards the builder and
/// falls back to [`TypedColumn::Fallback`].
pub struct TypedColumnBuilder {
    buf: TypedColumnBuilderBuf,
}

enum TypedColumnBuilderBuf {
    I64(Vec<i64>),
    F64(Vec<f64>),
    I32(Vec<i32>),
    Bool(Vec<bool>),
    Date(Vec<i64>),
    DateTime(Vec<i64>),
    Decimal(Vec<Decimal128Value>),
    Utf8(Vec<Arc<str>>),
}

impl TypedColumnBuilder {
    /// Create a buffer pre-sized for `len` rows of `kind`.
    pub fn with_capacity(kind: TypedKind, len: usize) -> Self {
        let buf = match kind {
            TypedKind::I64 => TypedColumnBuilderBuf::I64(Vec::with_capacity(len)),
            TypedKind::F64 => TypedColumnBuilderBuf::F64(Vec::with_capacity(len)),
            TypedKind::I32 => TypedColumnBuilderBuf::I32(Vec::with_capacity(len)),
            TypedKind::Bool => TypedColumnBuilderBuf::Bool(Vec::with_capacity(len)),
            TypedKind::Date => TypedColumnBuilderBuf::Date(Vec::with_capacity(len)),
            TypedKind::DateTime => TypedColumnBuilderBuf::DateTime(Vec::with_capacity(len)),
            TypedKind::Decimal => TypedColumnBuilderBuf::Decimal(Vec::with_capacity(len)),
            TypedKind::Utf8 => TypedColumnBuilderBuf::Utf8(Vec::with_capacity(len)),
        };
        Self { buf }
    }

    /// Push one value into the buffer.
    ///
    /// See [`PushOutcome`]: a matching value or a NULL slot pushes a
    /// placeholder (the caller tracks validity in the bitmap); a kind
    /// mismatch signals the caller to fall back to `Fallback`.
    pub fn push_value(&mut self, value: &Value) -> PushOutcome {
        match &mut self.buf {
            TypedColumnBuilderBuf::I64(buf) => match value {
                Value::BigInt(v) => {
                    buf.push(*v);
                    PushOutcome::Value
                }
                Value::Null(NullType::Null) => {
                    buf.push(0);
                    PushOutcome::Null
                }
                _ => PushOutcome::Mismatch,
            },
            TypedColumnBuilderBuf::F64(buf) => match value {
                Value::Double(v) => {
                    buf.push(*v);
                    PushOutcome::Value
                }
                Value::Null(NullType::Null) => {
                    buf.push(0.0);
                    PushOutcome::Null
                }
                _ => PushOutcome::Mismatch,
            },
            TypedColumnBuilderBuf::I32(buf) => match value {
                Value::Int(v) => {
                    buf.push(*v);
                    PushOutcome::Value
                }
                Value::Null(NullType::Null) => {
                    buf.push(0);
                    PushOutcome::Null
                }
                _ => PushOutcome::Mismatch,
            },
            TypedColumnBuilderBuf::Bool(buf) => match value {
                Value::Bool(v) => {
                    buf.push(*v);
                    PushOutcome::Value
                }
                Value::Null(NullType::Null) => {
                    buf.push(false);
                    PushOutcome::Null
                }
                _ => PushOutcome::Mismatch,
            },
            TypedColumnBuilderBuf::Date(buf) => match value {
                Value::Date(v) => {
                    buf.push(v.to_days());
                    PushOutcome::Value
                }
                Value::Null(NullType::Null) => {
                    buf.push(0);
                    PushOutcome::Null
                }
                _ => PushOutcome::Mismatch,
            },
            TypedColumnBuilderBuf::DateTime(buf) => match value {
                Value::DateTime(v) => {
                    buf.push(v.to_micros());
                    PushOutcome::Value
                }
                Value::Null(NullType::Null) => {
                    buf.push(0);
                    PushOutcome::Null
                }
                _ => PushOutcome::Mismatch,
            },
            TypedColumnBuilderBuf::Decimal(buf) => match value {
                Value::Decimal128(v) => {
                    buf.push(v.clone());
                    PushOutcome::Value
                }
                Value::Null(NullType::Null) => {
                    buf.push(Decimal128Value::from_i64(0));
                    PushOutcome::Null
                }
                _ => PushOutcome::Mismatch,
            },
            TypedColumnBuilderBuf::Utf8(buf) => match value {
                Value::String(v) => {
                    buf.push(Arc::from(v.as_str()));
                    PushOutcome::Value
                }
                Value::Null(NullType::Null) => {
                    buf.push(Arc::from(""));
                    PushOutcome::Null
                }
                _ => PushOutcome::Mismatch,
            },
        }
    }

    /// Estimated heap bytes of the value buffer (for memory accounting).
    pub fn estimated_bytes(&self) -> usize {
        match &self.buf {
            TypedColumnBuilderBuf::I64(v) => v.capacity() * std::mem::size_of::<i64>(),
            TypedColumnBuilderBuf::F64(v) => v.capacity() * std::mem::size_of::<f64>(),
            TypedColumnBuilderBuf::I32(v) => v.capacity() * std::mem::size_of::<i32>(),
            TypedColumnBuilderBuf::Bool(v) => v.capacity() * std::mem::size_of::<bool>(),
            TypedColumnBuilderBuf::Date(v) => v.capacity() * std::mem::size_of::<i64>(),
            TypedColumnBuilderBuf::DateTime(v) => v.capacity() * std::mem::size_of::<i64>(),
            TypedColumnBuilderBuf::Decimal(v) => {
                v.capacity() * std::mem::size_of::<Decimal128Value>()
            }
            TypedColumnBuilderBuf::Utf8(v) => v
                .iter()
                .map(|s| s.len() + std::mem::size_of::<Arc<str>>())
                .sum(),
        }
    }

    /// Convert into a typed column; `has_null` decides between the plain and
    /// `Nullable*` variant (the validity `bitmap` is kept either way).
    pub fn finish(self, has_null: bool, bitmap: Vec<u64>) -> TypedColumn {
        match self.buf {
            TypedColumnBuilderBuf::I64(buf) => {
                if has_null {
                    TypedColumn::NullableI64(buf, bitmap)
                } else {
                    TypedColumn::I64(buf)
                }
            }
            TypedColumnBuilderBuf::F64(buf) => {
                if has_null {
                    TypedColumn::NullableF64(buf, bitmap)
                } else {
                    TypedColumn::F64(buf)
                }
            }
            TypedColumnBuilderBuf::I32(buf) => {
                if has_null {
                    TypedColumn::NullableI32(buf, bitmap)
                } else {
                    TypedColumn::I32(buf)
                }
            }
            TypedColumnBuilderBuf::Bool(buf) => {
                if has_null {
                    TypedColumn::NullableBool(buf, bitmap)
                } else {
                    TypedColumn::Bool(buf)
                }
            }
            TypedColumnBuilderBuf::Date(buf) => {
                if has_null {
                    TypedColumn::NullableDate(buf, bitmap)
                } else {
                    TypedColumn::Date(buf)
                }
            }
            TypedColumnBuilderBuf::DateTime(buf) => {
                if has_null {
                    TypedColumn::NullableDateTime(buf, bitmap)
                } else {
                    TypedColumn::DateTime(buf)
                }
            }
            TypedColumnBuilderBuf::Decimal(buf) => {
                if has_null {
                    TypedColumn::NullableDecimal(buf, bitmap)
                } else {
                    TypedColumn::Decimal(buf)
                }
            }
            TypedColumnBuilderBuf::Utf8(buf) => {
                if has_null {
                    TypedColumn::NullableUtf8(buf, bitmap)
                } else {
                    TypedColumn::Utf8(buf)
                }
            }
        }
    }
}

/// Replicate a literal into a raw batch of `n` rows, when the literal has a
/// typed scalar kind (BigInt/Double/Int/Bool/Date/DateTime/String/Decimal128).
pub fn typed_literal_batch(value: &Value, n: usize) -> Option<TypedBatch> {
    value_to_kind(value)?;
    let fill = match value {
        Value::BigInt(v) => TypedBatch::I64(vec![*v; n]),
        Value::Double(v) => TypedBatch::F64(vec![*v; n]),
        Value::Int(v) => TypedBatch::I32(vec![*v; n]),
        Value::Bool(v) => TypedBatch::Bool(vec![*v; n]),
        Value::Date(v) => TypedBatch::Date(vec![v.to_days(); n]),
        Value::DateTime(v) => TypedBatch::DateTime(vec![v.to_micros(); n]),
        Value::String(v) => TypedBatch::Utf8(vec![Arc::from(v.as_str()); n]),
        Value::Decimal128(v) => TypedBatch::Decimal(vec![v.clone(); n]),
        _ => return None, // guarded by value_to_kind above
    };
    Some(fill)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::value::date_time::{DateTimeValue, DateValue};
    use graphdb_core::value::list::List;

    #[test]
    fn test_value_to_kind_mapping() {
        assert_eq!(value_to_kind(&Value::BigInt(1)), Some(TypedKind::I64));
        assert_eq!(value_to_kind(&Value::Double(1.0)), Some(TypedKind::F64));
        assert_eq!(value_to_kind(&Value::Int(1)), Some(TypedKind::I32));
        assert_eq!(value_to_kind(&Value::Bool(true)), Some(TypedKind::Bool));
        assert_eq!(
            value_to_kind(&Value::Date(DateValue::from_days(0))),
            Some(TypedKind::Date)
        );
        assert_eq!(
            value_to_kind(&Value::DateTime(DateTimeValue::default())),
            Some(TypedKind::DateTime)
        );
        assert_eq!(
            value_to_kind(&Value::String("s".into())),
            Some(TypedKind::Utf8)
        );
        assert_eq!(
            value_to_kind(&Value::Decimal128(Decimal128Value::from_i64(1))),
            Some(TypedKind::Decimal)
        );
    }

    #[test]
    fn test_value_to_kind_returns_none_without_fast_path() {
        assert_eq!(value_to_kind(&Value::Null(NullType::Null)), None);
        assert_eq!(value_to_kind(&Value::List(Box::new(List::new()))), None);
        assert_eq!(value_to_kind(&Value::Map(Box::default())), None);
    }

    #[test]
    fn test_first_non_null_probe() {
        let column = [
            Value::Null(NullType::Null),
            Value::Null(NullType::Null),
            Value::BigInt(7),
            Value::BigInt(8),
        ];
        assert_eq!(
            first_non_null(column.iter()),
            Some(&Value::BigInt(7)),
            "probe skips leading NULLs"
        );
        let all_null = [Value::Null(NullType::Null), Value::Null(NullType::Null)];
        assert_eq!(
            first_non_null(all_null.iter()),
            None,
            "all-NULL column has no probe"
        );
    }

    #[test]
    fn test_builder_kind_mismatch_errors() {
        let mut builder = TypedColumnBuilder::with_capacity(TypedKind::I64, 2);
        assert!(matches!(
            builder.push_value(&Value::BigInt(1)),
            PushOutcome::Value
        ));
        assert!(matches!(
            builder.push_value(&Value::Null(NullType::Null)),
            PushOutcome::Null
        ));
        assert!(matches!(
            builder.push_value(&Value::Double(1.0)),
            PushOutcome::Mismatch
        ));
    }

    #[test]
    fn test_builder_finish_nullable() {
        let mut builder = TypedColumnBuilder::with_capacity(TypedKind::I64, 2);
        let mut bitmap = vec![0u64; 1];
        assert!(matches!(
            builder.push_value(&Value::BigInt(1)),
            PushOutcome::Value
        ));
        bitmap[0] |= 1u64 << 0;
        assert!(matches!(
            builder.push_value(&Value::Null(NullType::Null)),
            PushOutcome::Null
        ));
        let column = builder.finish(true, bitmap);
        assert_eq!(column.value_at(0), Some(Value::BigInt(1)));
        assert_eq!(column.value_at(1), Some(Value::Null(NullType::Null)));
    }

    #[test]
    fn test_builder_datetime_and_decimal() {
        let dt = DateTimeValue {
            year: 2024,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            sec: 5,
            microsec: 6,
        };
        let mut builder = TypedColumnBuilder::with_capacity(TypedKind::DateTime, 1);
        assert!(matches!(
            builder.push_value(&Value::DateTime(dt.clone())),
            PushOutcome::Value
        ));
        assert_eq!(
            builder.finish(false, vec![]).value_at(0),
            Some(Value::DateTime(dt)),
            "DateTime round-trips through micros"
        );

        let d = Decimal128Value::from_i64(12345);
        let mut builder = TypedColumnBuilder::with_capacity(TypedKind::Decimal, 1);
        assert!(matches!(
            builder.push_value(&Value::Decimal128(d.clone())),
            PushOutcome::Value
        ));
        assert_eq!(
            builder.finish(false, vec![]).value_at(0),
            Some(Value::Decimal128(d)),
            "Decimal round-trips through the clone path"
        );
    }

    #[test]
    fn test_literal_batch_matches_kinds() {
        assert!(matches!(
            typed_literal_batch(&Value::BigInt(7), 3),
            Some(TypedBatch::I64(v)) if v == vec![7, 7, 7]
        ));
        let dt = DateTimeValue::default();
        assert!(matches!(
            typed_literal_batch(&Value::DateTime(dt), 2),
            Some(TypedBatch::DateTime(v)) if v == vec![0, 0]
        ));
        assert!(matches!(
            typed_literal_batch(&Value::Decimal128(Decimal128Value::from_i64(5)), 2),
            Some(TypedBatch::Decimal(v)) if v == vec![Decimal128Value::from_i64(5); 2]
        ));
        assert!(typed_literal_batch(&Value::Null(NullType::Null), 2).is_none());
    }
}
