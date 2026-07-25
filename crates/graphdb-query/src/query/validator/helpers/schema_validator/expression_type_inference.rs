use crate::core::types::expr::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::validator::ValueType;

use super::schema_lookup::SchemaValidator;

impl SchemaValidator {
    pub fn infer_expression_type(
        &self,
        expr: &Expression,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
        input_columns: &std::collections::HashMap<String, ValueType>,
    ) -> ValueType {
        match expr {
            Expression::Literal(value) => Self::value_to_value_type(value),
            Expression::Variable(name) => input_columns
                .get(name)
                .cloned()
                .unwrap_or(ValueType::Unknown),
            Expression::Property { object, property } => {
                self.infer_property_type(object, property, space_name, available_vars)
            }
            Expression::Binary { op, .. } => match op {
                crate::core::types::operators::BinaryOperator::Add
                | crate::core::types::operators::BinaryOperator::Subtract
                | crate::core::types::operators::BinaryOperator::Multiply
                | crate::core::types::operators::BinaryOperator::Divide => ValueType::Float,
                crate::core::types::operators::BinaryOperator::Equal
                | crate::core::types::operators::BinaryOperator::NotEqual
                | crate::core::types::operators::BinaryOperator::LessThan
                | crate::core::types::operators::BinaryOperator::LessThanOrEqual
                | crate::core::types::operators::BinaryOperator::GreaterThan
                | crate::core::types::operators::BinaryOperator::GreaterThanOrEqual
                | crate::core::types::operators::BinaryOperator::And
                | crate::core::types::operators::BinaryOperator::Or => ValueType::Bool,
                _ => ValueType::Unknown,
            },
            Expression::Unary { op, .. } => match op {
                crate::core::types::operators::UnaryOperator::Not => ValueType::Bool,
                crate::core::types::operators::UnaryOperator::Minus => ValueType::Float,
                _ => ValueType::Unknown,
            },
            Expression::Function { name, .. } => Self::infer_function_return_type(name),
            Expression::List(_) => ValueType::List,
            Expression::Map(_) => ValueType::Map,
            Expression::Case {
                conditions,
                default,
                ..
            } => {
                if !conditions.is_empty() {
                    let (_, result) = &conditions[0];
                    self.infer_expression_type(result, space_name, available_vars, input_columns)
                } else if let Some(def) = default {
                    self.infer_expression_type(def, space_name, available_vars, input_columns)
                } else {
                    ValueType::Unknown
                }
            }
            _ => ValueType::Unknown,
        }
    }

    fn infer_property_type(
        &self,
        object: &Expression,
        property: &str,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
    ) -> ValueType {
        let schema_name = match object {
            Expression::Variable(var_name) => {
                available_vars.get(var_name).cloned().unwrap_or_default()
            }
            Expression::Label(label_name) => label_name.clone(),
            _ => return ValueType::Unknown,
        };

        if schema_name.is_empty() {
            return ValueType::Unknown;
        }

        let properties = if let Ok(Some(tag_info)) =
            self.get_schema_manager().get_tag(space_name, &schema_name)
        {
            tag_info.properties
        } else if let Ok(Some(edge_info)) = self
            .get_schema_manager()
            .get_edge_type(space_name, &schema_name)
        {
            edge_info.properties
        } else {
            return ValueType::Unknown;
        };

        for prop in &properties {
            if prop.name == property {
                return Self::data_type_to_value_type(&prop.data_type);
            }
        }

        ValueType::Unknown
    }

    fn value_to_value_type(value: &Value) -> ValueType {
        match value {
            Value::Null(_) => ValueType::Null,
            Value::Bool(_) => ValueType::Bool,
            Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_) => ValueType::Int,
            Value::Float(_) | Value::Double(_) => ValueType::Float,
            Value::String(_) => ValueType::String,
            Value::Date(_) => ValueType::Date,
            Value::Time(_) => ValueType::Time,
            Value::DateTime(_) => ValueType::DateTime,
            Value::Vertex(_) => ValueType::Vertex,
            Value::Edge(_) => ValueType::Edge,
            Value::Path(_) => ValueType::Path,
            Value::List(_) => ValueType::List,
            Value::Map(_) => ValueType::Map,
            Value::Set(_) => ValueType::Set,
            _ => ValueType::Unknown,
        }
    }

    fn infer_function_return_type(function_name: &str) -> ValueType {
        match function_name.to_lowercase().as_str() {
            "count" | "sum" | "avg" | "min" | "max" => ValueType::Int,
            "size" | "length" => ValueType::Int,
            "contains" | "startswith" | "endswith" | "haskey" => ValueType::Bool,
            "substr" | "lower" | "upper" | "trim" | "ltrim" | "rtrim" | "replace" => {
                ValueType::String
            }
            "abs" | "round" | "floor" | "ceil" | "sqrt" | "log" | "exp" | "pow" => ValueType::Float,
            "type" | "label" => ValueType::String,
            "id" => ValueType::Int,
            "head" | "last" => ValueType::Unknown,
            "keys" | "labels" | "properties" => ValueType::List,
            "coalesce" => ValueType::Unknown,
            "nullif" => ValueType::Unknown,
            _ => ValueType::Unknown,
        }
    }

    pub fn infer_contextual_expression_type(
        &self,
        expr: &ContextualExpression,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
        input_columns: &std::collections::HashMap<String, ValueType>,
    ) -> ValueType {
        if let Some(inner_expr) = expr.get_expression() {
            self.infer_expression_type(&inner_expr, space_name, available_vars, input_columns)
        } else {
            ValueType::Unknown
        }
    }
}
