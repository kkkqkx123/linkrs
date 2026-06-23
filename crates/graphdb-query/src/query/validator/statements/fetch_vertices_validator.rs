//! Vertex Acquisition Validator – New System Version
//! Verify the FETCH PROP ON ... statement
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to provide a unified interface.
//! 2. All functions are retained.
//! Verify Lifecycle Management
//! Management of input/output columns
//! Expression property tracing
//! User-defined variable management
//! Permission check
//! Execution plan generation
//! 3. The lifecycle parameters have been removed, and the SchemaManager is now managed using Arc.
//! 4. Use QueryContext to manage the context in a unified manner.

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::operators::UnaryOperator;
use crate::core::Expression;
use crate::core::Value;
use crate::query::parser::ast::stmt::{Ast, FetchStmt, FetchTarget};
use crate::query::parser::ast::Stmt;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified vertex information
#[derive(Debug, Clone)]
pub struct ValidatedFetchVertices {
    pub space_id: u64,
    pub tag_names: Vec<String>,
    pub tag_ids: Vec<i32>,
    pub vertex_ids: Vec<Value>,
    pub yield_columns: Vec<ValidatedYieldColumn>,
    pub is_system: bool,
}

/// The verified YIELD column
#[derive(Debug, Clone)]
pub struct ValidatedYieldColumn {
    pub expression: ContextualExpression,
    pub alias: String,
    pub tag_name: Option<String>,
    pub prop_name: Option<String>,
}

/// Vertex Acquisition Validator – New System Implementation
///
/// Functionality integrity assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracing
/// 4. Management of user-defined variables
/// 5. Permission checking (scalable)
/// 6. Generation of execution plans (scalable)
#[derive(Debug)]
pub struct FetchVerticesValidator {
    // Schema management
    schema_manager: Option<Arc<SchemaManager>>,
    // Input column definition
    inputs: Vec<ColumnDef>,
    // Column definition
    outputs: Vec<ColumnDef>,
    // Expression properties
    expr_props: ExpressionProps,
    // User-defined variables
    user_defined_vars: Vec<String>,
    // Cache validation results
    validated_result: Option<ValidatedFetchVertices>,
}

impl FetchVerticesValidator {
    pub fn new() -> Self {
        Self {
            schema_manager: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validated_result: None,
        }
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    /// Obtain the verification results.
    pub fn validated_result(&self) -> Option<&ValidatedFetchVertices> {
        self.validated_result.as_ref()
    }

    /// Basic validation
    fn validate_fetch_vertices(
        &self,
        stmt: &FetchStmt,
        space_name: Option<&str>,
    ) -> Result<(), ValidationError> {
        match &stmt.target {
            FetchTarget::Vertices {
                tag_name,
                ids,
                properties,
            } => {
                // Validate tag exists if specified
                if let Some(ref tag) = tag_name {
                    self.validate_tag_exists(tag, space_name)?;
                }
                self.validate_vertex_ids(ids, space_name)?;
                self.validate_properties_clause(properties.as_ref())?;
                Ok(())
            }
            _ => Err(ValidationError::new(
                "Expected FETCH VERTICES statement".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Validate that the tag exists in the schema
    fn validate_tag_exists(
        &self,
        tag_name: &str,
        space_name: Option<&str>,
    ) -> Result<(), ValidationError> {
        if let (Some(ref schema_manager), Some(space)) = (&self.schema_manager, space_name) {
            match schema_manager.get_tag(space, tag_name) {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err(ValidationError::new(
                    format!("Tag '{}' not found in space '{}'", tag_name, space),
                    ValidationErrorType::SemanticError,
                )),
                Err(e) => Err(ValidationError::new(
                    format!("Failed to get tag '{}': {}", tag_name, e),
                    ValidationErrorType::SemanticError,
                )),
            }
        } else {
            // Without schema_manager, we can't validate, so just pass
            Ok(())
        }
    }

    /// Verify the list of vertex IDs.
    fn validate_vertex_ids(
        &self,
        vertex_ids: &[ContextualExpression],
        space_name: Option<&str>,
    ) -> Result<(), ValidationError> {
        if vertex_ids.is_empty() {
            return Err(ValidationError::new(
                "At least one vertex ID must be specified.".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        for vertex_id in vertex_ids {
            self.validate_vertex_id(vertex_id, space_name)?;
        }

        Ok(())
    }

    /// Verify a single vertex ID
    /// Using the unified validation method of SchemaValidator
    fn validate_vertex_id(
        &self,
        expr: &ContextualExpression,
        space_name: Option<&str>,
    ) -> Result<(), ValidationError> {
        // Get vid_type from schema_manager if available, otherwise default to String
        let vid_type = if let (Some(ref schema_manager), Some(space_name)) =
            (&self.schema_manager, space_name)
        {
            match schema_manager.get_space(space_name) {
                Ok(Some(space_info)) => space_info.vid_type,
                _ => crate::core::types::DataType::String,
            }
        } else {
            crate::core::types::DataType::String
        };

        if let Some(ref schema_manager) = self.schema_manager {
            let schema_validator =
                crate::query::validator::SchemaValidator::new(schema_manager.clone());
            schema_validator
                .validate_vid_expr(expr, &vid_type, "vertex")
                .map_err(|e| ValidationError::new(e.message, e.error_type))?
        } else {
            // Performing basic validation in the absence of the schema_manager
            Self::basic_validate_vertex_id(expr)?;
        }

        Ok(())
    }

    /// Basic vertex ID verification (when no SchemaManager is available)
    fn basic_validate_vertex_id(expr: &ContextualExpression) -> Result<(), ValidationError> {
        if expr.expression().is_none() {
            return Err(ValidationError::new(
                "Vertex ID expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if expr.is_variable() {
            return Ok(());
        }

        if expr.is_literal() {
            if let Some(value) = expr.as_literal() {
                if value.is_null() || value.is_empty() {
                    return Err(ValidationError::new(
                        "Vertex ID cannot be null".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                if let Value::String(s) = value {
                    if s.is_empty() {
                        return Err(ValidationError::new(
                            "Vertex ID cannot be null".to_string(),
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
                return Ok(());
            }
        }

        // Check for Unary(Minus, Literal(Int)) or Unary(Minus, Literal(BigInt))
        if let Some(meta) = expr.expression() {
            if let Expression::Unary {
                op: UnaryOperator::Minus,
                operand,
            } = meta.inner()
            {
                match operand.as_ref() {
                    Expression::Literal(Value::Int(_)) => return Ok(()),
                    Expression::Literal(Value::BigInt(_)) => return Ok(()),
                    _ => {}
                }
            }
        }

        Err(ValidationError::new(
            "Vertex ID must be a constant or variable".to_string(),
            ValidationErrorType::SemanticError,
        ))
    }

    /// Verify the list of attribute clauses.
    fn validate_properties_clause(
        &self,
        properties: Option<&Vec<String>>,
    ) -> Result<(), ValidationError> {
        // The attribute list can be empty, which indicates that all attributes should be retrieved.
        if let Some(props) = properties {
            let mut prop_set = std::collections::HashSet::new();
            for prop in props {
                if !prop_set.insert(prop) {
                    return Err(ValidationError::new(
                        format!("Attribute '{}' repeated", prop),
                        ValidationErrorType::DuplicateKey,
                    ));
                }
            }
        }
        Ok(())
    }

    /// The expression is evaluated to the value “Value”.
    fn evaluate_expression(&self, expr: &ContextualExpression) -> Result<Value, ValidationError> {
        let expr_meta = match expr.expression() {
            Some(m) => m,
            None => {
                return Err(ValidationError::new(
                    "Invalid expression".to_string(),
                    ValidationErrorType::SemanticError,
                ))
            }
        };
        let inner = expr_meta.inner();

        match inner {
            Expression::Literal(v) => Ok(v.clone()),
            Expression::Variable(name) => Ok(Value::String(format!("${}", name))),
            _ => Err(ValidationError::new(
                "Expressions must be constants or variables".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }
}

impl Default for FetchVerticesValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for FetchVerticesValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // 1. Check whether additional space is needed.
        if !self.is_global_statement() && qctx.space_id().is_none() {
            return Err(ValidationError::new(
                "No image space selected, please execute first USE <space>".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // 2. Obtain the FETCH statement
        let fetch_stmt = match &ast.stmt {
            Stmt::Fetch(fetch_stmt) => fetch_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected FETCH statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 3. Perform basic validation.
        let space_name = qctx.space_name();
        self.validate_fetch_vertices(fetch_stmt, space_name.as_deref())?;

        // 4. Obtain the space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 5. Extract vertex information
        let (tag_name, vertex_ids, properties) = match &fetch_stmt.target {
            FetchTarget::Vertices {
                tag_name,
                ids,
                properties,
            } => (tag_name.clone(), ids, properties),
            _ => {
                return Err(ValidationError::new(
                    "Expected FETCH VERTICES statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 6. Verify and convert the vertex IDs.
        let mut validated_vids = Vec::new();
        for vid_expr in vertex_ids {
            let vid = self.evaluate_expression(vid_expr)?;
            validated_vids.push(vid);
        }

        // 7. Verify and convert the attribute column to the YIELD column.
        let mut validated_columns = Vec::new();
        if let Some(props) = properties {
            for prop in props {
                // Creating a ContextualExpression to represent the attribute name
                let expr_meta = crate::core::types::expr::ExpressionMeta::new(
                    crate::core::Expression::Variable(prop.clone()),
                );
                let id = ast.expr_context.register_expression(expr_meta);
                let ctx_expr = crate::core::types::expr::contextual::ContextualExpression::new(
                    id,
                    ast.expr_context.clone(),
                );
                validated_columns.push(ValidatedYieldColumn {
                    expression: ctx_expr,
                    alias: prop.clone(),
                    tag_name: tag_name.clone(),
                    prop_name: Some(prop.clone()),
                });
            }
        }

        // 8. Create the verification results
        let tag_names: Vec<String> = tag_name.iter().cloned().collect();
        let validated = ValidatedFetchVertices {
            space_id,
            tag_names,
            tag_ids: vec![],
            vertex_ids: validated_vids,
            yield_columns: validated_columns,
            is_system: false,
        };

        // 9. Setting the output columns
        self.outputs.clear();
        for (i, col) in validated.yield_columns.iter().enumerate() {
            let col_name = if col.alias.is_empty() {
                format!("column_{}", i)
            } else {
                col.alias.clone()
            };
            self.outputs.push(ColumnDef {
                name: col_name,
                type_: ValueType::String,
            });
        }

        self.validated_result = Some(validated);

        // 10. Constructing detailed ValidationInfo
        let mut info = ValidationInfo::new();

        // Add semantic information
        info.semantic_info
            .referenced_tags
            .push("vertex".to_string());

        // 11. Return the verification results containing detailed information.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::FetchVertices
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `FETCH VERTICES` command is not a global statement; it is necessary to select a specific space in advance.
        false
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Expression;
    use crate::query::parser::ast::stmt::{FetchStmt, FetchTarget};
    use crate::query::parser::ast::Span;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;

    fn create_contextual_expr(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    fn _create_fetch_vertices_stmt(
        vertex_ids: Vec<Expression>,
        properties: Option<Vec<String>>,
    ) -> FetchStmt {
        FetchStmt {
            span: Span::default(),
            target: FetchTarget::Vertices {
                tag_name: None,
                ids: vertex_ids.into_iter().map(create_contextual_expr).collect(),
                properties,
            },
        }
    }

    #[test]
    fn test_validate_vertex_ids_empty() {
        let validator = FetchVerticesValidator::new();
        let result = validator.validate_vertex_ids(&[], None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message, "At least one vertex ID must be specified.");
    }

    #[test]
    fn test_validate_vertex_ids_valid() {
        let validator = FetchVerticesValidator::new();
        let vertex_ids = vec![
            Expression::Literal(Value::String("v1".to_string())),
            Expression::Literal(Value::String("v2".to_string())),
        ];
        let result = validator.validate_vertex_ids(
            &vertex_ids
                .into_iter()
                .map(create_contextual_expr)
                .collect::<Vec<_>>(),
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_vertex_ids_with_variable() {
        let validator = FetchVerticesValidator::new();
        let vertex_ids = vec![Expression::Variable("vids".to_string())];
        let result = validator.validate_vertex_ids(
            &vertex_ids
                .into_iter()
                .map(create_contextual_expr)
                .collect::<Vec<_>>(),
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_vertex_id_empty() {
        let validator = FetchVerticesValidator::new();
        let vertex_ids = vec![
            Expression::Literal(Value::String("v1".to_string())),
            Expression::Literal(Value::String("".to_string())),
        ];
        let result = validator.validate_vertex_ids(
            &vertex_ids
                .into_iter()
                .map(create_contextual_expr)
                .collect::<Vec<_>>(),
            None,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("cannot be null"));
    }

    #[test]
    fn test_validate_properties_clause_duplicate() {
        let validator = FetchVerticesValidator::new();
        let properties = Some(vec!["prop1".to_string(), "prop1".to_string()]);
        let result = validator.validate_properties_clause(properties.as_ref());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("repeated"));
    }

    #[test]
    fn test_validate_properties_clause_valid() {
        let validator = FetchVerticesValidator::new();
        let properties = Some(vec!["prop1".to_string(), "prop2".to_string()]);
        let result = validator.validate_properties_clause(properties.as_ref());
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_properties_clause_empty() {
        let validator = FetchVerticesValidator::new();
        let result = validator.validate_properties_clause(None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_statement_validator_trait() {
        let validator = FetchVerticesValidator::new();

        // Testing the `statement_type`
        assert_eq!(validator.statement_type(), StatementType::FetchVertices);

        // Testing inputs/outputs
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());

        // Testing user_defined_vars
        assert!(validator.user_defined_vars().is_empty());
    }
}
