//! Expression type derivation
//!
//! Provide expression type derivation functions.

use crate::core::types::expr::Expression;
use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use crate::core::types::DataType;
use crate::core::Value;
use crate::core::{ArrayTypeInfo, StructTypeInfo};
use std::sync::Arc;

impl Expression {
    /// Deriving the data type of an expression
    ///
    /// Derive the return type of an expression from its structure and operators.
    /// Returns DataType::Empty if the type cannot be determined.
    pub fn deduce_type(&self) -> DataType {
        match self {
            Expression::Literal(value) => Self::deduce_value_type(value),
            Expression::Variable(_) => DataType::Empty,
            Expression::Property { .. } => DataType::Empty,
            Expression::StructField { base, field } => Self::deduce_struct_field_type(base, field),
            Expression::Binary { op, left, right } => Self::deduce_binary_type(op, left, right),
            Expression::Unary { op, operand } => Self::deduce_unary_type(op, operand),
            Expression::Function { name, args } => Self::deduce_function_type(name, args),
            Expression::Aggregate { func, .. } => Self::deduce_aggregate_type(func),
            Expression::List(_) => DataType::List,
            Expression::Map(_) => DataType::Map,
            Expression::Case {
                conditions,
                default,
                ..
            } => Self::deduce_case_type(conditions, default.as_deref()),
            Expression::TypeCast { target_type, .. } => target_type.clone(),
            Expression::Subscript { collection, .. } => Self::deduce_subscript_type(collection),
            Expression::Range { .. } => DataType::List,
            Expression::Path(_) => DataType::Path,
            Expression::Label(_) => DataType::String,
            Expression::ListComprehension { .. } => DataType::List,
            Expression::LabelTagProperty { .. } => DataType::Empty,
            Expression::TagProperty { .. } => DataType::Empty,
            Expression::EdgeProperty { .. } => DataType::Empty,
            Expression::Predicate { .. } => DataType::Bool,
            Expression::Reduce { .. } => DataType::Empty,
            Expression::PathBuild(_) => DataType::Path,
            Expression::Parameter(_) => DataType::Empty,
            Expression::SessionVariable(_) => DataType::Empty,
            Expression::Vector(_) => DataType::Vector,
            Expression::Exists { .. } => DataType::Bool,
            Expression::In { .. } => DataType::Bool,
            Expression::WindowFunction { .. } => DataType::Empty,
        }
    }

    /// Deriving value types
    ///
    /// Exhaustive over all `Value` variants: adding a new variant without a
    /// branch here fails to compile (no `_ => DataType::Empty` fallback).
    fn deduce_value_type(value: &Value) -> DataType {
        match value {
            Value::Empty => DataType::Empty,
            Value::Null(_) => DataType::Null,
            Value::Bool(_) => DataType::Bool,
            Value::SmallInt(_) => DataType::SmallInt,
            Value::Int(_) => DataType::Int,
            Value::BigInt(_) => DataType::BigInt,
            Value::Float(_) => DataType::Float,
            Value::Double(_) => DataType::Double,
            Value::Decimal128(_) => DataType::Decimal128,
            Value::String(_) => DataType::String,
            Value::FixedString { len, .. } => DataType::FixedString(*len),
            Value::Blob(_) => DataType::Blob,
            Value::Date(_) => DataType::Date,
            Value::Time(_) => DataType::Time,
            Value::DateTime(_) => DataType::DateTime,
            Value::Vertex(_) => DataType::Vertex,
            Value::Edge(_) => DataType::Edge,
            Value::Path(_) => DataType::Path,
            Value::List(_) => DataType::List,
            Value::Map(_) => DataType::Map,
            Value::Set(_) => DataType::Set,
            Value::Geography(_) => DataType::Geography,
            Value::Vector(v) => DataType::VectorDense(v.dimension()),
            Value::DataSet(_) => DataType::DataSet,
            Value::Json(_) => DataType::Json,
            Value::JsonB(_) => DataType::JsonB,
            Value::Uuid(_) => DataType::Uuid,
            Value::Interval(_) => DataType::Interval,
            Value::VertexId(_) => DataType::Vertex,
            Value::EdgeId(_) => DataType::Edge,
            Value::Struct(s) => DataType::Struct(Arc::new(StructTypeInfo::new(
                s.fields
                    .iter()
                    .map(|(name, value)| (name.clone(), value.get_type()))
                    .collect(),
            ))),
            Value::Array(a) => DataType::Array(Arc::new(ArrayTypeInfo::new(
                a.values
                    .first()
                    .map(|v| v.get_type())
                    .unwrap_or(DataType::Empty),
                None,
            ))),
        }
    }

    /// Derive the type of a STRUCT field access `base.field`.
    ///
    /// Without schema context the base type is unknown, so this returns
    /// `DataType::Empty` (the binder fills the type after schema resolution,
    /// matching the `Property` semantics).
    fn deduce_struct_field_type(base: &Expression, field: &str) -> DataType {
        match base.deduce_type() {
            DataType::Struct(info) => info
                .fields
                .iter()
                .find(|(name, _)| name == field)
                .map(|(_, field_type)| field_type.clone())
                .unwrap_or(DataType::Empty),
            _ => DataType::Empty,
        }
    }

    /// Deriving binary operation types
    fn deduce_binary_type(op: &BinaryOperator, left: &Expression, right: &Expression) -> DataType {
        match op {
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Modulo
            | BinaryOperator::Exponent => {
                let left_type = left.deduce_type();
                let right_type = right.deduce_type();
                Self::deduce_arithmetic_type(&left_type, &right_type)
            }
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual
            | BinaryOperator::And
            | BinaryOperator::Or
            | BinaryOperator::Xor
            | BinaryOperator::Like
            | BinaryOperator::In
            | BinaryOperator::NotIn
            | BinaryOperator::Contains
            | BinaryOperator::StartsWith
            | BinaryOperator::EndsWith => DataType::Bool,
            BinaryOperator::StringConcat => DataType::String,
            _ => DataType::Empty,
        }
    }

    /// Deriving arithmetic operation result types
    ///
    /// Reuses the numeric promotion hierarchy of `TypeUtils::get_common_type`
    /// so this cannot drift from the executor-level type computation.
    fn deduce_arithmetic_type(left: &DataType, right: &DataType) -> DataType {
        crate::core::type_system::TypeUtils::get_common_type(left, right)
    }

    /// Derive the type of unary operation
    fn deduce_unary_type(op: &UnaryOperator, operand: &Expression) -> DataType {
        match op {
            UnaryOperator::Not => DataType::Bool,
            UnaryOperator::IsNull | UnaryOperator::IsNotNull => DataType::Bool,
            UnaryOperator::IsEmpty | UnaryOperator::IsNotEmpty => DataType::Bool,
            UnaryOperator::Plus | UnaryOperator::Minus => operand.deduce_type(),
        }
    }

    /// Deriving function return types
    fn deduce_function_type(name: &str, args: &[Expression]) -> DataType {
        let name_upper = name.to_uppercase();
        match name_upper.as_str() {
            // math function
            "ABS" | "CEIL" | "FLOOR" | "ROUND" | "SIGN" | "SQRT" | "POW" | "EXP" | "LOG"
            | "LOG10" | "LOG2" => {
                if let Some(first_arg) = args.first() {
                    first_arg.deduce_type()
                } else {
                    DataType::Empty
                }
            }
            // string function
            "LENGTH" | "SIZE" => DataType::Int,
            "SUBSTRING" | "REPLACE" | "TRIM" | "LTRIM" | "RTRIM" | "UPPER" | "LOWER" | "CONCAT" => {
                DataType::String
            }
            // type conversion function
            "TOSTRING" => DataType::String,
            "TOINT" => DataType::Int,
            "TOFLOAT" => DataType::Float,
            "TOBOOLEAN" => DataType::Bool,
            // aggregate function (math.)
            "HEAD" | "LAST" => {
                if let Some(first_arg) = args.first() {
                    first_arg.deduce_type()
                } else {
                    DataType::Empty
                }
            }
            "TAIL" | "NODES" | "RELATIONSHIPS" | "KEYS" | "LABELS" | "RANGE" => DataType::List,
            // Aggregation Related Functions
            "COUNT" => DataType::Int,
            "COLLECT" => DataType::List,
            // Graph Related Functions
            "ID" | "SRC" | "DST" | "TYPE" => DataType::String,
            "STARTNODE" | "ENDNODE" => DataType::Vertex,
            // time function
            "NOW" | "TIMESTAMP" => DataType::DateTime,
            "DATE" => DataType::Date,
            "TIME" => DataType::Time,
            // conditional function
            "COALESCE" => {
                // Returns the type of the first non-null argument
                for arg in args {
                    let arg_type = arg.deduce_type();
                    if arg_type != DataType::Null && arg_type != DataType::Empty {
                        return arg_type;
                    }
                }
                DataType::Empty
            }
            _ => DataType::Empty,
        }
    }

    /// Deriving Aggregate Function Return Types
    fn deduce_aggregate_type(func: &AggregateFunction) -> DataType {
        match func {
            AggregateFunction::Count => DataType::Int,
            AggregateFunction::Sum => DataType::Float,
            AggregateFunction::Avg => DataType::Float,
            AggregateFunction::Min => DataType::Empty,
            AggregateFunction::Max => DataType::Empty,
            AggregateFunction::Collect => DataType::List,
            AggregateFunction::CollectSet => DataType::List,
            AggregateFunction::Percentile => DataType::Float,
            AggregateFunction::Std => DataType::Float,
            AggregateFunction::StddevPop => DataType::Float,
            AggregateFunction::StddevSamp => DataType::Float,
            AggregateFunction::Product => DataType::Float,
            AggregateFunction::PercentileCont => DataType::Float,
            AggregateFunction::Variance => DataType::Float,
            AggregateFunction::Median => DataType::Float,
            AggregateFunction::Mode => DataType::Empty,
            AggregateFunction::BitAnd => DataType::Int,
            AggregateFunction::BitOr => DataType::Int,
            AggregateFunction::BoolAnd => DataType::Bool,
            AggregateFunction::BoolOr => DataType::Bool,
            AggregateFunction::GroupConcat => DataType::String,
            AggregateFunction::GroupConcatWithOrder => DataType::String,
            AggregateFunction::VecSum => DataType::Vector,
            AggregateFunction::VecAvg => DataType::Vector,
        }
    }

    /// Deriving Conditional Expression Types
    fn deduce_case_type(
        conditions: &[(Expression, Expression)],
        default: Option<&Expression>,
    ) -> DataType {
        // Trying to derive types from conditional branches
        for (_, value) in conditions {
            let value_type = value.deduce_type();
            if value_type != DataType::Empty {
                return value_type;
            }
        }
        // Trying to derive types from the default branch
        if let Some(def) = default {
            def.deduce_type()
        } else {
            DataType::Empty
        }
    }

    /// Deriving subscript access types
    fn deduce_subscript_type(collection: &Expression) -> DataType {
        let collection_type = collection.deduce_type();
        match collection_type {
            DataType::List => DataType::Empty,
            DataType::Map => DataType::Empty,
            DataType::String => DataType::String,
            DataType::Path => DataType::Vertex,
            _ => DataType::Empty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::{EdgeId, VertexId};
    use crate::core::value::date_time::{DateTimeValue, DateValue, TimeValue};
    use crate::core::value::decimal128::Decimal128Value;
    use crate::core::value::geography::Geography;
    use crate::core::value::interval::IntervalValue;
    use crate::core::value::json::{Json, JsonB};
    use crate::core::value::list::List;
    use crate::core::value::null::NullType;
    use crate::core::value::uuid::UuidValue;
    use crate::core::vertex_edge_path::{Edge, Path, Vertex};
    use crate::core::DataSet;
    use std::collections::HashMap;

    fn literal(value: Value) -> Expression {
        Expression::Literal(value)
    }

    #[test]
    fn test_deduce_value_type_exhaustive() {
        // One case per Value variant; a new variant fails to compile here.
        let cases = vec![
            (Value::Empty, DataType::Empty),
            (Value::Null(NullType::Null), DataType::Null),
            (Value::Bool(true), DataType::Bool),
            (Value::SmallInt(1), DataType::SmallInt),
            (Value::Int(1), DataType::Int),
            (Value::BigInt(1), DataType::BigInt),
            (Value::Float(1.0), DataType::Float),
            (Value::Double(1.0), DataType::Double),
            (
                Value::Decimal128(Decimal128Value::from_i64(1)),
                DataType::Decimal128,
            ),
            (Value::string("s"), DataType::String),
            (
                Value::fixed_string(4, "ab".to_string()),
                DataType::FixedString(4),
            ),
            (Value::Blob(vec![1]), DataType::Blob),
            (
                Value::Date(DateValue {
                    year: 2026,
                    month: 1,
                    day: 1,
                }),
                DataType::Date,
            ),
            (
                Value::Time(TimeValue {
                    hour: 1,
                    minute: 2,
                    sec: 3,
                    microsec: 4,
                }),
                DataType::Time,
            ),
            (
                Value::DateTime(DateTimeValue {
                    year: 2026,
                    month: 1,
                    day: 1,
                    hour: 1,
                    minute: 2,
                    sec: 3,
                    microsec: 4,
                }),
                DataType::DateTime,
            ),
            (
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
                DataType::Vertex,
            ),
            (
                Value::Edge(Box::new(Edge::new_empty(
                    VertexId::from_int64(1),
                    VertexId::from_int64(2),
                    "E".to_string(),
                    0,
                ))),
                DataType::Edge,
            ),
            (
                Value::Path(Box::new(Path::new(Vertex::with_vid(VertexId::from_int64(
                    1,
                ))))),
                DataType::Path,
            ),
            (
                Value::list(List {
                    values: vec![Value::Int(1)],
                }),
                DataType::List,
            ),
            (
                Value::string_map(HashMap::from([("k".to_string(), Value::Int(1))])),
                DataType::Map,
            ),
            (
                Value::set(std::collections::HashSet::from([Value::Int(1)])),
                DataType::Set,
            ),
            (
                Value::Geography(Geography::from_wkt("POINT(1 2)").expect("wkt")),
                DataType::Geography,
            ),
            (Value::vector(vec![1.0, 2.0]), DataType::VectorDense(2)),
            (
                Value::DataSet(Box::new(DataSet::from_rows(
                    vec![vec![Value::Int(1)]],
                    vec!["c".to_string()],
                ))),
                DataType::DataSet,
            ),
            (
                Value::Json(Box::new(Json::parse("{}").expect("json"))),
                DataType::Json,
            ),
            (
                Value::JsonB(Box::new(JsonB::parse("{}").expect("jsonb"))),
                DataType::JsonB,
            ),
            (Value::Uuid(UuidValue([0u8; 16])), DataType::Uuid),
            (
                Value::Interval(IntervalValue::new(1, 2, 3)),
                DataType::Interval,
            ),
            (Value::VertexId(VertexId::from_int64(1)), DataType::Vertex),
            (Value::EdgeId(EdgeId::new(1)), DataType::Edge),
            (
                Value::struct_(vec![("city".to_string(), Value::string("x"))]),
                DataType::Struct(Arc::new(StructTypeInfo::new(vec![(
                    "city".to_string(),
                    DataType::String,
                )]))),
            ),
            (
                Value::array(vec![Value::Double(1.0)]),
                DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::Double, None))),
            ),
        ];

        for (value, expected) in cases {
            assert_eq!(
                Expression::deduce_type(&literal(value)),
                expected,
                "deduced type for literal must match"
            );
        }
    }

    #[test]
    fn test_deduce_arithmetic_type_reuses_promotion() {
        assert_eq!(
            Expression::literal(Value::Int(1)).deduce_type(),
            DataType::Int
        );
        // Int + Float promotes to Float.
        let expr = Expression::Binary {
            op: BinaryOperator::Add,
            left: Box::new(literal(Value::Int(1))),
            right: Box::new(literal(Value::Float(1.0))),
        };
        assert_eq!(expr.deduce_type(), DataType::Float);

        // BigInt + Decimal128 promotes to Decimal128.
        let expr = Expression::Binary {
            op: BinaryOperator::Add,
            left: Box::new(literal(Value::BigInt(1))),
            right: Box::new(literal(Value::Decimal128(Decimal128Value::from_i64(1)))),
        };
        assert_eq!(expr.deduce_type(), DataType::Decimal128);

        // Decimal128 + Float promotes to Double.
        let expr = Expression::Binary {
            op: BinaryOperator::Multiply,
            left: Box::new(literal(Value::Decimal128(Decimal128Value::from_i64(1)))),
            right: Box::new(literal(Value::Float(1.0))),
        };
        assert_eq!(expr.deduce_type(), DataType::Double);

        // String + Int has no common type.
        let expr = Expression::Binary {
            op: BinaryOperator::Add,
            left: Box::new(literal(Value::string("a"))),
            right: Box::new(literal(Value::Int(1))),
        };
        assert_eq!(expr.deduce_type(), DataType::Empty);
    }
}
