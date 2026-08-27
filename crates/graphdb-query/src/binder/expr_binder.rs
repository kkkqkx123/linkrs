//! Expression-level type deduction for the Binder.
//!
//! This module provides **stateless** type deduction: given an `Expression`
//! and the current binding scope, it returns the resolved `DataType`.
//!
//! Unlike the Validator's `ExpressionAnalyzer`, it does **not** mutate the
//! `ExpressionAnalysisContext`. It is a pure function from `Expression` →
//! `DataType`, designed to be called from `Binder::bind_inner_expr()`.

use crate::core::types::expr::Expression;
use crate::core::types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
use crate::core::DataType;
use crate::binder::scope::BinderScope;

/// Stateless expression type deduction.
pub struct ExpressionBinder<'a> {
    scope: &'a BinderScope,
}

impl<'a> ExpressionBinder<'a> {
    pub fn new(scope: &'a BinderScope) -> Self {
        Self { scope }
    }

    /// Resolve the type of an expression.
    pub fn resolve_type(&self, expr: &Expression) -> DataType {
        match expr {
            Expression::Literal(v) => v.data_type(),

            Expression::Variable(name) => self
                .scope
                .lookup(name)
                .and_then(|v| v.properties.values().next().map(|vt| vt.to_data_type()))
                .unwrap_or(DataType::String),

            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    self.resolve_property_type(var_name, property)
                } else {
                    DataType::String
                }
            }

            Expression::StructField { base, field } => match self.resolve_type(base) {
                DataType::Struct(info) => info
                    .fields
                    .iter()
                    .find(|(name, _)| name == field)
                    .map(|(_, field_type)| field_type.clone())
                    .unwrap_or(DataType::String),
                _ => DataType::String,
            },

            Expression::Binary { op, left, right } => {
                let left_type = self.resolve_type(left);
                let right_type = self.resolve_type(right);
                match op {
                    BinaryOperator::Equal
                    | BinaryOperator::NotEqual
                    | BinaryOperator::LessThan
                    | BinaryOperator::LessThanOrEqual
                    | BinaryOperator::GreaterThan
                    | BinaryOperator::GreaterThanOrEqual
                    | BinaryOperator::And
                    | BinaryOperator::Or => DataType::Bool,
                    BinaryOperator::StringConcat => DataType::String,
                    BinaryOperator::Add
                    | BinaryOperator::Subtract
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide
                    | BinaryOperator::Modulo
                    | BinaryOperator::Exponent => {
                        self.deduce_arithmetic_type(&left_type, &right_type)
                    }
                    _ => DataType::Unknown,
                }
            }

            Expression::Unary { op, .. } => match op {
                UnaryOperator::Not => DataType::Bool,
                UnaryOperator::IsNull | UnaryOperator::IsNotNull => DataType::Bool,
                UnaryOperator::Plus | UnaryOperator::Minus => DataType::Float,
                UnaryOperator::IsEmpty | UnaryOperator::IsNotEmpty => DataType::Bool,
            },

            Expression::Function { name, args } => {
                let arg_types: Vec<DataType> = args.iter().map(|a| self.resolve_type(a)).collect();
                self.deduce_function_return_type(name, &arg_types)
            }

            Expression::Aggregate { func, args, .. } => {
                let arg_type = args
                    .first()
                    .map(|a| self.resolve_type(a))
                    .unwrap_or(DataType::Unknown);
                self.deduce_aggregate_return_type(func, &arg_type)
            }

            Expression::List(items) => {
                DataType::List(Box::new(self.resolve_container_element(items)))
            }
            Expression::Map(entries) => {
                DataType::Map(Box::new(self.resolve_map_value_type(entries)))
            }

            Expression::Case {
                conditions,
                default,
                ..
            } => {
                for (_, value) in conditions {
                    let t = self.resolve_type(value);
                    if t != DataType::Unknown {
                        return t;
                    }
                }
                default
                    .as_ref()
                    .map(|d| self.resolve_type(d))
                    .unwrap_or(DataType::String)
            }

            Expression::Subscript { collection, .. } => {
                let _ = collection;
                DataType::String
            }

            Expression::TypeCast { target_type, .. } => target_type.clone(),

            Expression::ListComprehension { .. } => DataType::List(Box::new(DataType::Unknown)),
            // The REDUCE result type is the accumulator type, determined by
            // the initial value and the mapping expression.
            Expression::Reduce {
                initial, mapping, ..
            } => {
                let initial_type = self.resolve_type(initial);
                let mapping_type = self.resolve_type(mapping);
                if initial_type != DataType::Unknown {
                    initial_type
                } else if mapping_type != DataType::Unknown {
                    mapping_type
                } else {
                    DataType::Unknown
                }
            }
            // Window functions derive their return type from the underlying
            // function name and argument types (same table as scalar calls).
            Expression::WindowFunction { name, args, .. } => {
                let arg_types: Vec<DataType> = args.iter().map(|a| self.resolve_type(a)).collect();
                self.deduce_function_return_type(name, &arg_types)
            }

            Expression::Parameter(_) => DataType::String,
            Expression::SessionVariable(_) => DataType::Unknown,
            Expression::Vector(v) => DataType::VectorDense(v.len()),

            Expression::Path(_) => DataType::List(Box::new(DataType::Unknown)),
            Expression::PathBuild(_) => DataType::List(Box::new(DataType::Unknown)),
            Expression::Label(_) => DataType::String,
            Expression::TagProperty { .. } => DataType::String,
            Expression::EdgeProperty { .. } => DataType::String,
            Expression::LabelTagProperty { .. } => DataType::String,
            Expression::Predicate { .. } => DataType::Bool,
            Expression::Range { collection, .. } => match self.resolve_type(collection) {
                DataType::List(element) => DataType::List(element),
                DataType::Array(info) => DataType::Array(info),
                _ => DataType::List(Box::new(DataType::Unknown)),
            },
            Expression::Exists { .. } => DataType::Bool,
            Expression::In { .. } => DataType::Bool,
        }
    }

    /// Resolve property type from scope, falling back to String.
    pub fn resolve_property_type(&self, var_name: &str, property: &str) -> DataType {
        if let Some(var_info) = self.scope.lookup(var_name) {
            if let Some(vt) = var_info.properties.get(property) {
                return vt.to_data_type();
            }
        }
        DataType::String
    }

    /// Common element type of an `Expression::List` literal (falls back to
    /// `Unknown` for an empty/heterogeneous list).
    fn resolve_container_element(&self, items: &[Expression]) -> DataType {
        let mut common = DataType::Unknown;
        for item in items {
            let item_type = self.resolve_type(item);
            common = if common == DataType::Unknown {
                item_type
            } else {
                crate::core::type_system::TypeUtils::get_common_type(&common, &item_type)
            };
            if common == DataType::Empty {
                return DataType::Unknown;
            }
        }
        common
    }

    /// Common value type of an `Expression::Map` literal (falls back to
    /// `Unknown` for an empty/heterogeneous map).
    fn resolve_map_value_type(&self, entries: &[(String, Expression)]) -> DataType {
        let mut common = DataType::Unknown;
        for (_, value) in entries {
            let value_type = self.resolve_type(value);
            common = if common == DataType::Unknown {
                value_type
            } else {
                crate::core::type_system::TypeUtils::get_common_type(&common, &value_type)
            };
            if common == DataType::Empty {
                return DataType::Unknown;
            }
        }
        common
    }

    /// Arithmetic type promotion: int + float → float.
    pub fn deduce_arithmetic_type(&self, left: &DataType, right: &DataType) -> DataType {
        let left_is_numeric = matches!(
            left,
            DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Float
                | DataType::Double
        );
        let right_is_numeric = matches!(
            right,
            DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Float
                | DataType::Double
        );

        if !left_is_numeric || !right_is_numeric {
            return DataType::Unknown;
        }

        let left_is_float = matches!(left, DataType::Float | DataType::Double);
        let right_is_float = matches!(right, DataType::Float | DataType::Double);

        if left_is_float || right_is_float {
            DataType::Float
        } else {
            DataType::Int
        }
    }

    /// Deduce function return type from name and argument types.
    pub fn deduce_function_return_type(&self, name: &str, arg_types: &[DataType]) -> DataType {
        match name.to_lowercase().as_str() {
            "abs" | "length" | "size" | "round" | "floor" | "ceil" => DataType::Int,
            "sqrt" | "pow" | "sin" | "cos" | "tan" => DataType::Float,
            "concat" | "substring" | "trim" | "ltrim" | "rtrim" | "upper" | "lower" | "type" => {
                DataType::String
            }
            "id" => DataType::Int,
            "properties" => DataType::Map(Box::new(DataType::Empty)),
            "labels" | "keys" => DataType::List(Box::new(DataType::String)),
            "values" | "range" | "reverse" => DataType::List(Box::new(DataType::Empty)),
            "toboolean" | "toBoolean" => DataType::Bool,
            "tointeger" | "toInteger" => DataType::Int,
            "tofloat" | "toFloat" => DataType::Float,
            "tostring" | "toString" => DataType::String,
            "head" => arg_types.first().cloned().unwrap_or(DataType::String),
            "last" => arg_types.first().cloned().unwrap_or(DataType::String),
            "coalesce" => arg_types.first().cloned().unwrap_or(DataType::String),
            _ => DataType::String,
        }
    }

    /// Deduce aggregate function return type.
    pub fn deduce_aggregate_return_type(
        &self,
        func: &AggregateFunction,
        arg_type: &DataType,
    ) -> DataType {
        match func {
            AggregateFunction::Count => DataType::Int,
            AggregateFunction::Sum => DataType::Float,
            AggregateFunction::Avg => DataType::Float,
            AggregateFunction::Max | AggregateFunction::Min => arg_type.clone(),
            AggregateFunction::Collect => DataType::List(Box::new(arg_type.clone())),
            AggregateFunction::CollectSet => DataType::Set(Box::new(arg_type.clone())),
            AggregateFunction::Percentile => DataType::Float,
            AggregateFunction::PercentileCont => DataType::Float,
            AggregateFunction::Std => DataType::Float,
            AggregateFunction::StddevPop => DataType::Float,
            AggregateFunction::StddevSamp => DataType::Float,
            AggregateFunction::Product => DataType::Float,
            AggregateFunction::Variance => DataType::Float,
            AggregateFunction::Median => DataType::Float,
            AggregateFunction::Mode => arg_type.clone(),
            AggregateFunction::BitAnd | AggregateFunction::BitOr => DataType::Int,
            AggregateFunction::BoolAnd | AggregateFunction::BoolOr => DataType::Bool,
            AggregateFunction::GroupConcat => DataType::String,
            AggregateFunction::GroupConcatWithOrder => DataType::String,
            AggregateFunction::VecSum => DataType::Vector,
            AggregateFunction::VecAvg => DataType::Vector,
        }
    }
}
