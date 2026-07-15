use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::{EdgeTypeInfo, PropertyDef, TagInfo};
use crate::query::validator::error::{ValidationError as CoreValidationError, ValidationErrorType};

#[derive(Debug, Clone)]
pub struct SchemaValidator {
    schema_manager: Arc<SchemaManager>,
}

impl SchemaValidator {
    pub fn new(schema_manager: Arc<SchemaManager>) -> Self {
        Self { schema_manager }
    }

    pub fn get_schema_manager(&self) -> &SchemaManager {
        self.schema_manager.as_ref()
    }

    pub fn schema_manager_arc(&self) -> Arc<SchemaManager> {
        self.schema_manager.clone()
    }

    pub fn get_tag(
        &self,
        space_name: &str,
        tag_name: &str,
    ) -> Result<Option<TagInfo>, CoreValidationError> {
        self.schema_manager
            .as_ref()
            .get_tag(space_name, tag_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Tag: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })
    }

    pub fn get_edge_type(
        &self,
        space_name: &str,
        edge_type_name: &str,
    ) -> Result<Option<EdgeTypeInfo>, CoreValidationError> {
        self.schema_manager
            .as_ref()
            .get_edge_type(space_name, edge_type_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Edge Type: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })
    }

    pub fn get_all_edge_types(
        &self,
        space_name: &str,
    ) -> Result<Vec<EdgeTypeInfo>, CoreValidationError> {
        self.schema_manager
            .as_ref()
            .list_edge_types(space_name)
            .map_err(|e| {
                CoreValidationError::new(
                    format!("Failed to get Edge Type list: {}", e),
                    ValidationErrorType::SemanticError,
                )
            })
    }

    pub fn get_property_def<'b>(
        &self,
        prop_name: &str,
        properties: &'b [PropertyDef],
    ) -> Option<&'b PropertyDef> {
        properties.iter().find(|p| p.name == prop_name)
    }

    pub fn validate_expression_properties(
        &self,
        expr: &Expression,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
    ) -> Result<(), CoreValidationError> {
        match expr {
            Expression::Property { object, property } => {
                self.validate_property_reference(object, property, space_name, available_vars)
            }
            Expression::Binary { left, right, .. } => {
                self.validate_expression_properties(left, space_name, available_vars)?;
                self.validate_expression_properties(right, space_name, available_vars)
            }
            Expression::Unary { operand, .. } => {
                self.validate_expression_properties(operand, space_name, available_vars)
            }
            Expression::Function { args, .. } => {
                for arg in args {
                    self.validate_expression_properties(arg, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::List(items) => {
                for item in items {
                    self.validate_expression_properties(item, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::Map(map) => {
                for (_, value) in map {
                    self.validate_expression_properties(value, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test) = test_expr {
                    self.validate_expression_properties(test, space_name, available_vars)?;
                }
                for (condition, result) in conditions {
                    self.validate_expression_properties(condition, space_name, available_vars)?;
                    self.validate_expression_properties(result, space_name, available_vars)?;
                }
                if let Some(def) = default {
                    self.validate_expression_properties(def, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::Aggregate { args, .. } => {
                for arg in args {
                    self.validate_expression_properties(arg, space_name, available_vars)?;
                }
                Ok(())
            }
            Expression::Predicate { args, .. } => {
                for arg in args {
                    self.validate_expression_properties(arg, space_name, available_vars)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn validate_property_reference(
        &self,
        object: &Expression,
        property: &str,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
    ) -> Result<(), CoreValidationError> {
        let schema_name = match object {
            Expression::Variable(var_name) => {
                available_vars.get(var_name).cloned().unwrap_or_default()
            }
            Expression::Label(label_name) => label_name.clone(),
            _ => {
                return Err(CoreValidationError::new(
                    format!(
                        "Invalid property access: property '{}' on non-variable object",
                        property
                    ),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        if schema_name.is_empty() {
            return Err(CoreValidationError::new(
                format!("Cannot determine schema for property access '{}'", property),
                ValidationErrorType::SemanticError,
            ));
        }

        if schema_name == "vertex" || schema_name == "Vertex" {
            return Ok(());
        }

        if schema_name == "edge" || schema_name == "Edge" {
            return Ok(());
        }

        let properties =
            if let Ok(Some(tag_info)) = self.schema_manager.get_tag(space_name, &schema_name) {
                tag_info.properties
            } else if let Ok(Some(edge_info)) =
                self.schema_manager.get_edge_type(space_name, &schema_name)
            {
                edge_info.properties
            } else {
                return Err(CoreValidationError::new(
                    format!(
                        "Schema '{}' not found in space '{}'",
                        schema_name, space_name
                    ),
                    ValidationErrorType::SemanticError,
                ));
            };

        if !properties.iter().any(|p| p.name == property) {
            return Err(CoreValidationError::new(
                format!(
                    "Property '{}' not found in schema '{}'",
                    property, schema_name
                ),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    pub fn validate_contextual_expression_properties(
        &self,
        expr: &ContextualExpression,
        space_name: &str,
        available_vars: &std::collections::HashMap<String, String>,
    ) -> Result<(), CoreValidationError> {
        if let Some(inner_expr) = expr.get_expression() {
            self.validate_expression_properties(&inner_expr, space_name, available_vars)
        } else {
            Err(CoreValidationError::new(
                "Invalid expression: unable to get expression content".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }
}
