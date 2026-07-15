use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::operators::UnaryOperator;
use crate::core::types::{DataType, PropertyDef};
use crate::core::Value;
use crate::query::validator::error::{ValidationError as CoreValidationError, ValidationErrorType};
use crate::query::validator::validator_trait::ValueType;

use super::schema_lookup::SchemaValidator;

impl SchemaValidator {
    pub fn validate_property_exists(
        &self,
        prop_name: &str,
        properties: &[PropertyDef],
    ) -> Result<(), CoreValidationError> {
        if !properties.iter().any(|p| p.name == prop_name) {
            return Err(CoreValidationError::new(
                format!("Attribute '{}' not present in Schema", prop_name),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    pub fn validate_property_type(
        &self,
        prop_name: &str,
        expected_type: &DataType,
        value: &Value,
    ) -> Result<(), CoreValidationError> {
        if matches!(value, Value::Null(_)) {
            return Ok(());
        }

        let actual_type = value.get_type();

        if !Self::is_type_compatible(expected_type, &actual_type) {
            return Err(CoreValidationError::new(
                format!(
                    "Attribute '{}' Desired type {:?} , actual type {:?}",
                    prop_name, expected_type, actual_type
                ),
                ValidationErrorType::TypeMismatch,
            ));
        }
        Ok(())
    }

    pub fn is_type_compatible(expected: &DataType, actual: &DataType) -> bool {
        match (expected, actual) {
            (a, b) if a == b => true,

            (DataType::SmallInt, DataType::Int) => true,
            (DataType::SmallInt, DataType::BigInt) => true,
            (DataType::Int, DataType::SmallInt) => true,
            (DataType::Int, DataType::BigInt) => true,
            (DataType::BigInt, DataType::SmallInt) => true,
            (DataType::BigInt, DataType::Int) => true,

            (DataType::Float, DataType::Double) => true,
            (DataType::Double, DataType::Float) => true,

            (DataType::VID, DataType::String) => true,
            (DataType::VID, DataType::SmallInt) => true,
            (DataType::VID, DataType::Int) => true,
            (DataType::VID, DataType::BigInt) => true,
            (DataType::VID, DataType::FixedString(_)) => true,

            (DataType::FixedString(_), DataType::String) => true,
            (DataType::String, DataType::FixedString(_)) => true,

            (_, DataType::Null) => true,

            _ => false,
        }
    }

    pub fn data_type_to_value_type(data_type: &DataType) -> ValueType {
        match data_type {
            DataType::Bool => ValueType::Bool,
            DataType::SmallInt | DataType::Int | DataType::BigInt => ValueType::Int,
            DataType::Float | DataType::Double => ValueType::Float,
            DataType::String | DataType::FixedString(_) => ValueType::String,
            DataType::Date => ValueType::Date,
            DataType::Time => ValueType::Time,
            DataType::DateTime => ValueType::DateTime,
            DataType::Null => ValueType::Null,
            DataType::Vertex => ValueType::Vertex,
            DataType::Edge => ValueType::Edge,
            DataType::Path => ValueType::Path,
            DataType::List => ValueType::List,
            DataType::Map => ValueType::Map,
            DataType::Set => ValueType::Set,
            _ => ValueType::Unknown,
        }
    }

    pub fn validate_not_null(
        &self,
        prop_name: &str,
        prop_def: &PropertyDef,
        value: &Value,
    ) -> Result<(), CoreValidationError> {
        if !prop_def.nullable && matches!(value, Value::Null(_)) {
            return Err(CoreValidationError::new(
                format!("The non-null attribute '{}' cannot be NULL.", prop_name),
                ValidationErrorType::ConstraintViolation,
            ));
        }
        Ok(())
    }

    pub fn get_default_value(&self, prop_def: &PropertyDef) -> Option<Value> {
        prop_def.default.clone()
    }

    pub fn fill_default_values(
        &self,
        properties: &[PropertyDef],
        provided_props: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, CoreValidationError> {
        let mut result = provided_props.to_vec();

        for prop_def in properties {
            if !result.iter().any(|(name, _)| name == &prop_def.name) {
                if let Some(default) = &prop_def.default {
                    result.push((prop_def.name.clone(), default.clone()));
                } else if !prop_def.nullable {
                    return Err(CoreValidationError::new(
                        format!(
                            "Attribute '{}' is not provided and has no default value, and is not allowed to be NULL.",
                            prop_def.name
                        ),
                        ValidationErrorType::ConstraintViolation,
                    ));
                } else {
                    result.push((
                        prop_def.name.clone(),
                        Value::Null(crate::core::NullType::default()),
                    ));
                }
            }
        }

        Ok(result)
    }

    pub fn validate_vid(
        &self,
        vid: &Value,
        expected_type: &DataType,
    ) -> Result<(), CoreValidationError> {
        match expected_type {
            DataType::String | DataType::FixedString(_) => {
                if !matches!(vid, Value::String(_)) {
                    return Err(CoreValidationError::new(
                        format!("VID Expected string type, actually {:?}", vid.get_type()),
                        ValidationErrorType::TypeMismatch,
                    ));
                }
            }
            DataType::SmallInt | DataType::Int | DataType::BigInt => {
                if !matches!(vid, Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_)) {
                    return Err(CoreValidationError::new(
                        format!("VID Expected integer type, actually {:?}", vid.get_type()),
                        ValidationErrorType::TypeMismatch,
                    ));
                }
            }
            DataType::VID => {
                if !matches!(
                    vid,
                    Value::String(_) | Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_)
                ) {
                    return Err(CoreValidationError::new(
                        format!("VID type incompatibility: {:?}", vid.get_type()),
                        ValidationErrorType::TypeMismatch,
                    ));
                }
            }
            _ => {
                return Err(CoreValidationError::new(
                    format!("Unsupported VID types: {:?}", expected_type),
                    ValidationErrorType::TypeMismatch,
                ));
            }
        }
        Ok(())
    }

    pub fn validate_vid_expr(
        &self,
        expr: &ContextualExpression,
        vid_type: &DataType,
        role: &str,
    ) -> Result<(), CoreValidationError> {
        if let Some(e) = expr.get_expression() {
            self.validate_vid_expr_internal(&e, vid_type, role)
        } else {
            Err(CoreValidationError::new(
                format!("{} vertex ID expression is invalid", role),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    fn validate_vid_expr_internal(
        &self,
        expr: &Expression,
        vid_type: &DataType,
        role: &str,
    ) -> Result<(), CoreValidationError> {
        match expr {
            Expression::Literal(value) => {
                match value {
                    Value::String(s) => {
                        if s.is_empty() {
                            return Err(CoreValidationError::new(
                                format!("{} vertex ID cannot be an empty string.", role),
                                ValidationErrorType::SemanticError,
                            ));
                        }
                        if !matches!(
                            vid_type,
                            DataType::String | DataType::FixedString(_) | DataType::VID
                        ) {
                            return Err(CoreValidationError::new(
                                format!(
                                    "{} vertex ID expects {:?} type, actually a string",
                                    role, vid_type
                                ),
                                ValidationErrorType::TypeMismatch,
                            ));
                        }
                    }
                    Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_) => {
                        if !matches!(
                            vid_type,
                            DataType::SmallInt | DataType::Int | DataType::BigInt | DataType::VID
                        ) {
                            return Err(CoreValidationError::new(
                                format!(
                                    "{} vertex ID expectation {:?} type, actually an integer",
                                    role, vid_type
                                ),
                                ValidationErrorType::TypeMismatch,
                            ));
                        }
                    }
                    _ => {
                        return Err(CoreValidationError::new(
                            format!("{} vertex ID must be a string or integer constant.", role),
                            ValidationErrorType::TypeMismatch,
                        ));
                    }
                }
                Ok(())
            }
            Expression::Variable(_) => Ok(()),
            Expression::Unary {
                op: UnaryOperator::Minus,
                operand,
            } => match operand.as_ref() {
                Expression::Literal(Value::SmallInt(_))
                | Expression::Literal(Value::Int(_))
                | Expression::Literal(Value::BigInt(_)) => {
                    if !matches!(
                        vid_type,
                        DataType::SmallInt | DataType::Int | DataType::BigInt | DataType::VID
                    ) {
                        return Err(CoreValidationError::new(
                            format!(
                                "{} vertex ID expectation {:?} type, actually a negative integer",
                                role, vid_type
                            ),
                            ValidationErrorType::TypeMismatch,
                        ));
                    }
                    Ok(())
                }
                _ => Err(CoreValidationError::new(
                    format!("{} vertex ID must be a constant or variable.", role),
                    ValidationErrorType::SemanticError,
                )),
            },
            _ => Err(CoreValidationError::new(
                format!("{} vertex ID must be a constant or variable.", role),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    pub fn validate_properties(
        &self,
        properties: &[PropertyDef],
        prop_values: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, CoreValidationError> {
        let mut result = Vec::new();

        for (prop_name, value) in prop_values {
            let prop_def = self
                .get_property_def(prop_name, properties)
                .ok_or_else(|| {
                    CoreValidationError::new(
                        format!("Attribute '{}' does not exist", prop_name),
                        ValidationErrorType::SemanticError,
                    )
                })?;

            self.validate_not_null(prop_name, prop_def, value)?;

            self.validate_property_type(prop_name, &prop_def.data_type, value)?;

            result.push((prop_name.clone(), value.clone()));
        }

        self.fill_default_values(properties, &result)
    }
}
