//! Expression type derivation
//!
//! Provide expression type derivation functions.

use crate::types::expr::Expression;
use crate::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use crate::types::DataType;
use crate::Value;
use crate::{ArrayTypeInfo, StructTypeInfo};
use std::sync::Arc;

impl Expression {
    /// Deriving the data type of an expression
    ///
    /// Derive the return type of an expression from its structure and operators.
    /// Returns `DataType::Unknown` if the type cannot be determined without
    /// schema/binding information (the binder completes these paths), and
    /// `DataType::Empty` only for the explicit `Value::Empty`.
    pub fn deduce_type(&self) -> DataType {
        match self {
            Expression::Literal(value) => Self::deduce_value_type(value),
            Expression::Variable(_) => DataType::Unknown,
            Expression::Property { .. } => DataType::Unknown,
            Expression::StructField { base, field } => Self::deduce_struct_field_type(base, field),
            Expression::Binary { op, left, right } => Self::deduce_binary_type(op, left, right),
            Expression::Unary { op, operand } => Self::deduce_unary_type(op, operand),
            Expression::Function { name, args } => Self::deduce_function_type(name, args),
            Expression::Aggregate { func, .. } => Self::deduce_aggregate_type(func),
            Expression::List(items) => {
                DataType::List(Box::new(Self::deduce_expression_container_element(items)))
            }
            Expression::Map(entries) => {
                DataType::Map(Box::new(Self::deduce_map_value_type(entries)))
            }
            Expression::Case {
                conditions,
                default,
                ..
            } => Self::deduce_case_type(conditions, default.as_deref()),
            Expression::TypeCast { target_type, .. } => target_type.clone(),
            Expression::Subscript { collection, .. } => Self::deduce_subscript_type(collection),
            Expression::Range { collection, .. } => Self::deduce_slice_type(collection),
            Expression::Path(_) => DataType::Path,
            Expression::Label(_) => DataType::String,
            Expression::ListComprehension { map, .. } => DataType::List(Box::new(
                map.as_deref()
                    .map(Self::deduce_type)
                    .unwrap_or(DataType::Unknown),
            )),
            Expression::LabelTagProperty { .. } => DataType::Unknown,
            Expression::TagProperty { .. } => DataType::Unknown,
            Expression::EdgeProperty { .. } => DataType::Unknown,
            Expression::Predicate { .. } => DataType::Bool,
            Expression::Reduce { .. } => DataType::Unknown,
            Expression::PathBuild(_) => DataType::Path,
            Expression::Parameter(_) => DataType::Unknown,
            Expression::SessionVariable(_) => DataType::Unknown,
            Expression::Vector(v) => DataType::VectorDense(v.len()),
            Expression::Exists { .. } => DataType::Bool,
            Expression::In { .. } => DataType::Bool,
            Expression::WindowFunction { .. } => DataType::Unknown,
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
            Value::FixedString(data) => DataType::FixedString(data.chars().count()),
            Value::Blob(_) => DataType::Blob,
            Value::Date(_) => DataType::Date,
            Value::Time(_) => DataType::Time,
            Value::DateTime(_) => DataType::DateTime,
            Value::Vertex(_) => DataType::Vertex,
            Value::Edge(_) => DataType::Edge,
            Value::Path(_) => DataType::Path,
            Value::List(_) | Value::Map(_) | Value::Set(_) => value.get_type(),
            Value::Geography(_) => DataType::Geography,
            Value::Vector(v) => DataType::VectorDense(v.dimension()),
            Value::DataSet(_) => DataType::DataSet,
            Value::Json(_) => DataType::Json,
            Value::JsonB(_) => DataType::JsonB,
            Value::Uuid(_) => DataType::Uuid,
            Value::Interval(_) => DataType::Interval,
            // Value::VertexId/EdgeId are internal executor optimizations that
            // substitute the full Vertex/Edge during traversal intermediate hops
            // (see expand_pushdown / graph_operator). They are semantically the
            // same entity as Vertex/Edge, so they deduce to the same DataType.
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
    /// `DataType::Unknown` (the binder fills the type after schema resolution,
    /// matching the `Property` semantics).
    fn deduce_struct_field_type(base: &Expression, field: &str) -> DataType {
        match base.deduce_type() {
            DataType::Struct(info) => info
                .fields
                .iter()
                .find(|(name, _)| name == field)
                .map(|(_, field_type)| field_type.clone())
                .unwrap_or(DataType::Unknown),
            _ => DataType::Unknown,
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
            _ => DataType::Unknown,
        }
    }

    /// Deriving arithmetic operation result types
    ///
    /// Reuses the numeric promotion hierarchy of `TypeUtils::get_common_type`
    /// so this cannot drift from the executor-level type computation.
    /// A missing common type (e.g. String + Int) is reported as `Unknown`
    /// rather than `Empty`; the binder/executor validates the operands.
    fn deduce_arithmetic_type(left: &DataType, right: &DataType) -> DataType {
        match crate::type_system::TypeUtils::get_common_type(left, right) {
            DataType::Empty => DataType::Unknown,
            common => common,
        }
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
                    DataType::Unknown
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
                    DataType::Unknown
                }
            }
            "TAIL" => {
                if let Some(first_arg) = args.first() {
                    DataType::List(Box::new(first_arg.deduce_type()))
                } else {
                    DataType::List(Box::new(DataType::Unknown))
                }
            }
            "NODES" | "RELATIONSHIPS" | "KEYS" | "LABELS" | "RANGE" => {
                DataType::List(Box::new(DataType::Empty))
            }
            // Aggregation Related Functions
            "COUNT" => DataType::Int,
            "COLLECT" => DataType::List(Box::new(DataType::Empty)),
            // Graph Related Functions
            "ID" | "SRC" | "DST" | "TYPE" => DataType::String,
            "STARTNODE" | "ENDNODE" => DataType::Vertex,
            // time function
            "NOW" | "TIMESTAMP" => DataType::DateTime,
            "DATE" => DataType::Date,
            "TIME" => DataType::Time,
            // conditional function
            "COALESCE" => {
                // Returns the type of the first argument with a known type
                for arg in args {
                    let arg_type = arg.deduce_type();
                    if arg_type != DataType::Null
                        && arg_type != DataType::Empty
                        && arg_type != DataType::Unknown
                    {
                        return arg_type;
                    }
                }
                DataType::Unknown
            }
            _ => DataType::Unknown,
        }
    }

    /// Deriving Aggregate Function Return Types
    fn deduce_aggregate_type(func: &AggregateFunction) -> DataType {
        match func {
            AggregateFunction::Count => DataType::Int,
            AggregateFunction::Sum => DataType::Float,
            AggregateFunction::Avg => DataType::Float,
            AggregateFunction::Min => DataType::Unknown,
            AggregateFunction::Max => DataType::Unknown,
            AggregateFunction::Collect => DataType::List(Box::new(DataType::Unknown)),
            AggregateFunction::CollectSet => DataType::Set(Box::new(DataType::Unknown)),
            AggregateFunction::Percentile => DataType::Float,
            AggregateFunction::Std => DataType::Float,
            AggregateFunction::StddevPop => DataType::Float,
            AggregateFunction::StddevSamp => DataType::Float,
            AggregateFunction::Product => DataType::Float,
            AggregateFunction::PercentileCont => DataType::Float,
            AggregateFunction::Variance => DataType::Float,
            AggregateFunction::Median => DataType::Float,
            AggregateFunction::Mode => DataType::Unknown,
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
            if value_type != DataType::Unknown {
                return value_type;
            }
        }
        // Trying to derive types from the default branch
        if let Some(def) = default {
            def.deduce_type()
        } else {
            DataType::Unknown
        }
    }

    /// Deriving subscript access types
    fn deduce_subscript_type(collection: &Expression) -> DataType {
        let collection_type = collection.deduce_type();
        match collection_type {
            DataType::List(element) => element.as_ref().clone(),
            DataType::Map(value) => value.as_ref().clone(),
            DataType::Set(element) => element.as_ref().clone(),
            DataType::Array(info) => info.element.as_ref().clone(),
            DataType::String => DataType::String,
            DataType::Path => DataType::Vertex,
            _ => DataType::Unknown,
        }
    }

    /// Element type of a LIST/MAP/ARRAY-typed expression, used for slicing
    /// (`Range` access preserves the collection element type).
    fn deduce_slice_type(collection: &Expression) -> DataType {
        match collection.deduce_type() {
            DataType::List(element) => DataType::List(element),
            DataType::Array(info) => DataType::Array(info),
            _ => DataType::List(Box::new(DataType::Unknown)),
        }
    }

    /// Common element type of an `Expression::List` literal (falls back to
    /// `Unknown` for an empty/heterogeneous list).
    fn deduce_expression_container_element(items: &[Expression]) -> DataType {
        let mut common = DataType::Unknown;
        for item in items {
            let item_type = item.deduce_type();
            common = if common == DataType::Unknown {
                item_type
            } else {
                crate::type_system::TypeUtils::get_common_type(&common, &item_type)
            };
            if common == DataType::Empty {
                return DataType::Unknown;
            }
        }
        common
    }

    /// Common value type of an `Expression::Map` literal (falls back to
    /// `Unknown` for an empty/heterogeneous map).
    fn deduce_map_value_type(entries: &[(String, Expression)]) -> DataType {
        let mut common = DataType::Unknown;
        for (_, value) in entries {
            let value_type = value.deduce_type();
            common = if common == DataType::Unknown {
                value_type
            } else {
                crate::type_system::TypeUtils::get_common_type(&common, &value_type)
            };
            if common == DataType::Empty {
                return DataType::Unknown;
            }
        }
        common
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::storage_ids::{EdgeId, VertexId};
    use crate::value::date_time::{DateTimeValue, DateValue, TimeValue};
    use crate::value::decimal128::Decimal128Value;
    use crate::value::geography::Geography;
    use crate::value::interval::IntervalValue;
    use crate::value::json::{Json, JsonB};
    use crate::value::list::List;
    use crate::value::null::NullType;
    use crate::value::uuid::UuidValue;
    use crate::vertex_edge_path::{Edge, Path, Vertex};
    use crate::DataSet;
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
                DataType::List(Box::new(DataType::Int)),
            ),
            (
                Value::string_map(HashMap::from([("k".to_string(), Value::Int(1))])),
                DataType::Map(Box::new(DataType::Int)),
            ),
            (
                Value::set(std::collections::HashSet::from([Value::Int(1)])),
                DataType::Set(Box::new(DataType::Int)),
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
        assert_eq!(expr.deduce_type(), DataType::Unknown);
    }

    #[test]
    fn test_deduce_vector_expression_matches_value_type() {
        // `Expression::Vector` deduces to `VectorDense(n)`,
        // aligned with `Value::Vector → VectorDense(dim)`. No dimension-less
        // `DataType::Vector` leak from the literal path.
        let expr = Expression::Vector(vec![1.0, 2.0, 3.0]);
        assert_eq!(expr.deduce_type(), DataType::VectorDense(3));
    }

    #[test]
    fn test_deduce_container_literals_carry_element_type() {
        // literal containers deduce with their element/value
        // type instead of the bare parameter-free `List`/`Map`/`Set`.
        let list = Expression::List(vec![literal(Value::Int(1)), literal(Value::Int(2))]);
        assert_eq!(list.deduce_type(), DataType::List(Box::new(DataType::Int)));
        // Heterogeneous numeric list promotes to the common supertype.
        let promoted = Expression::List(vec![literal(Value::Int(1)), literal(Value::Float(2.0))]);
        assert_eq!(
            promoted.deduce_type(),
            DataType::List(Box::new(DataType::Float))
        );
        // Empty / untyped list carries the `Unknown` element marker.
        let empty = Expression::List(vec![]);
        assert_eq!(
            empty.deduce_type(),
            DataType::List(Box::new(DataType::Unknown))
        );
        // Map value type is derived from the entry values.
        let map = Expression::Map(vec![("k".to_string(), literal(Value::Int(1)))]);
        assert_eq!(map.deduce_type(), DataType::Map(Box::new(DataType::Int)));
    }

    #[test]
    fn test_deduce_binding_dependent_expressions_are_unknown() {
        // Expressions that need schema/binding info report `Unknown`, never
        // `Empty` (which is reserved for the explicit `Value::Empty`).
        let cases: Vec<Expression> = vec![
            Expression::Variable("v".to_string()),
            Expression::Property {
                object: Box::new(literal(Value::string("v"))),
                property: "p".to_string(),
            },
            Expression::TagProperty {
                tag_name: "t".to_string(),
                property: "p".to_string(),
            },
            Expression::EdgeProperty {
                edge_name: "e".to_string(),
                property: "p".to_string(),
            },
            Expression::LabelTagProperty {
                tag: Box::new(literal(Value::string("t"))),
                property: "p".to_string(),
            },
            Expression::Parameter("$p".to_string()),
            Expression::SessionVariable("$$s".to_string()),
            Expression::Reduce {
                accumulator: "acc".to_string(),
                initial: Box::new(literal(Value::Int(0))),
                variable: "x".to_string(),
                source: Box::new(Expression::List(vec![])),
                mapping: Box::new(literal(Value::Int(1))),
            },
            Expression::WindowFunction {
                name: "row_number".to_string(),
                args: vec![],
                over_partition_by: vec![],
                over_order_by: vec![],
                over_order_desc: vec![],
            },
        ];
        for expr in cases {
            assert_eq!(
                expr.deduce_type(),
                DataType::Unknown,
                "binding-dependent expression must deduce to Unknown"
            );
        }
    }
}
