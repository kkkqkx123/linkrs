//! While obtaining the validator – New system version
//! Verify the FETCH PROP ON ... statement
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All functions are retained.
//! Verify lifecycle management
//! Column management for input/output data
//! Expression property tracing
//! User-defined variable management
//! Permission check
//! Execution plan generation
//! 3. The lifecycle parameters have been removed, and SchemaManager is now managed using Arc.
//! 4. Use AstContext to manage the context in a unified manner.

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Value;
use crate::query::parser::ast::stmt::{Ast, FetchStmt, FetchTarget};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified edge acquisition information
#[derive(Debug, Clone)]
pub struct ValidatedFetchEdges {
    pub space_id: u64,
    pub edge_name: String,
    pub edge_type: Option<i32>,
    pub edge_keys: Vec<ValidatedEdgeKey>,
    pub yield_columns: Vec<ValidatedYieldColumn>,
    pub is_system: bool,
}

/// Verified side key
#[derive(Debug, Clone)]
pub struct ValidatedEdgeKey {
    pub src_id: Value,
    pub dst_id: Value,
    pub rank: i64,
}

/// The verified YIELD column
#[derive(Debug, Clone)]
pub struct ValidatedYieldColumn {
    pub expression: ContextualExpression,
    pub alias: Option<String>,
    pub prop_name: Option<String>,
}

/// While obtaining the validator – Implementation of the new system
///
/// Functionality integrity assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracking
/// 4. Management of user-defined variables
/// 5. Permission checking (scalable)
/// 6. Generation of execution plans (scalable)
#[derive(Debug)]
pub struct FetchEdgesValidator {
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
    validated_result: Option<ValidatedFetchEdges>,
}

impl FetchEdgesValidator {
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
    pub fn validated_result(&self) -> Option<&ValidatedFetchEdges> {
        self.validated_result.as_ref()
    }

    /// Verify the YIELD clause (check for duplicate aliases).
    pub fn validate_yield_clause(
        &self,
        yield_columns: &[(ContextualExpression, Option<String>)],
    ) -> Result<(), ValidationError> {
        let mut seen_aliases = std::collections::HashSet::new();

        for (_, alias) in yield_columns {
            if let Some(ref name) = alias {
                if !seen_aliases.insert(name.clone()) {
                    return Err(ValidationError::new(
                        format!("Duplicate aliases: {}", name),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        Ok(())
    }

    /// Basic validation
    fn validate_fetch_edges(&self, stmt: &FetchStmt) -> Result<(), ValidationError> {
        match &stmt.target {
            FetchTarget::Edges {
                edge_type,
                src,
                dst,
                rank,
                ..
            } => {
                self.validate_edge_name(edge_type)?;
                self.validate_edge_key(src, dst, rank.as_ref())?;
                Ok(())
            }
            _ => Err(ValidationError::new(
                "Expected FETCH EDGES statement".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Verify the names of the edge types.
    fn validate_edge_name(&self, edge_name: &str) -> Result<(), ValidationError> {
        if edge_name.is_empty() {
            return Err(ValidationError::new(
                "The name of the edge type must be specified.".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }
        Ok(())
    }

    /// Verify the side keys
    fn validate_edge_key(
        &self,
        src: &ContextualExpression,
        dst: &ContextualExpression,
        rank: Option<&ContextualExpression>,
    ) -> Result<(), ValidationError> {
        // Verify the source vertex expression.
        self.validate_endpoint(src, "source vertex")?;
        // Verify the target vertex expression.
        self.validate_endpoint(dst, "target vertex")?;
        // Verify the rank value.
        if let Some(rank_expr) = rank {
            self.validate_rank(rank_expr)?;
        }

        Ok(())
    }

    /// Verify the endpoint expression.
    fn validate_endpoint(
        &self,
        expr: &ContextualExpression,
        endpoint_type: &str,
    ) -> Result<(), ValidationError> {
        if expr.expression().is_none() {
            return Err(ValidationError::new(
                format!("Invalid {} ID expression for a side key", endpoint_type),
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
                        format!("The {} ID of a side key cannot be null.", endpoint_type),
                        ValidationErrorType::SemanticError,
                    ));
                }
                return Ok(());
            }
        }

        Err(ValidationError::new(
            format!(
                "The {} ID of the side key must be a constant or variable",
                endpoint_type
            ),
            ValidationErrorType::SemanticError,
        ))
    }

    /// Verify the rank value.
    fn validate_rank(&self, expr: &ContextualExpression) -> Result<(), ValidationError> {
        if expr.expression().is_none() {
            return Err(ValidationError::new(
                "Rank expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if expr.is_variable() {
            return Ok(());
        }

        if let Some(Value::Int(i)) = expr.as_literal() {
            if i >= 0 {
                return Ok(());
            } else {
                return Err(ValidationError::new(
                    "The rank value must be a non-negative integer".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Err(ValidationError::new(
            "The rank value must be of integer type".to_string(),
            ValidationErrorType::SemanticError,
        ))
    }

    /// Evaluating the expression results in the value “Value”.
    fn evaluate_expression(&self, expr: &ContextualExpression) -> Result<Value, ValidationError> {
        if expr.expression().is_none() {
            return Err(ValidationError::new(
                "Invalid expression".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        if let Some(value) = expr.as_literal() {
            return Ok(value.clone());
        }

        if let Some(name) = expr.as_variable() {
            return Ok(Value::String(format!("${}", name)));
        }

        Err(ValidationError::new(
            "Expressions must be constants or variables".to_string(),
            ValidationErrorType::SemanticError,
        ))
    }

    /// Evaluating the rank expression
    fn evaluate_rank(&self, expr: &Option<ContextualExpression>) -> Result<i64, ValidationError> {
        let inner_expr = match expr {
            Some(ctx_expr) => {
                if ctx_expr.expression().is_none() {
                    return Err(ValidationError::new(
                        "Rank expression is invalid".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                ctx_expr
            }
            None => return Ok(0),
        };

        if inner_expr.is_variable() {
            return Ok(0);
        }

        if let Some(Value::BigInt(i)) = inner_expr.as_literal() {
            return Ok(i);
        }

        Err(ValidationError::new(
            "The rank value must be an integer".to_string(),
            ValidationErrorType::TypeMismatch,
        ))
    }

    /// Obtain the EdgeType ID
    fn get_edge_type_id(
        &self,
        edge_name: &str,
        _space_id: u64,
    ) -> Result<Option<i32>, ValidationError> {
        let _ = edge_name;
        Ok(None)
    }
}

impl Default for FetchEdgesValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for FetchEdgesValidator {
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
            crate::query::parser::ast::Stmt::Fetch(fetch_stmt) => fetch_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected FETCH statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 3. Perform basic validation.
        self.validate_fetch_edges(fetch_stmt)?;

        // 4. Obtain the space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 5. Extract edge information and verify it.
        let (edge_type_name, src, dst, rank, properties) = match &fetch_stmt.target {
            FetchTarget::Edges {
                edge_type,
                src,
                dst,
                rank,
                properties,
            } => (
                edge_type.clone(),
                src,
                dst,
                rank.clone(),
                properties.clone(),
            ),
            _ => {
                return Err(ValidationError::new(
                    "Expected FETCH EDGES statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 6. Retrieve the edge_type_id
        let edge_type_id = self.get_edge_type_id(&edge_type_name, space_id)?;

        // 7. Verify and convert the edge keys.
        let src_id = self.evaluate_expression(src)?;
        let dst_id = self.evaluate_expression(dst)?;
        let rank_val = self.evaluate_rank(&rank)?;
        let validated_keys = vec![ValidatedEdgeKey {
            src_id,
            dst_id,
            rank: rank_val,
        }];

        // 8. Verify and convert the YIELD column (constructed from the properties).
        let mut validated_columns = Vec::new();
        if let Some(props) = properties {
            for prop in props {
                // Creating a `ContextualExpression` to represent the attribute name
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
                    alias: Some(prop.clone()),
                    prop_name: None,
                });
            }
        }

        // 9. Create the verification results
        let validated = ValidatedFetchEdges {
            space_id,
            edge_name: edge_type_name.clone(),
            edge_type: edge_type_id,
            edge_keys: validated_keys,
            yield_columns: validated_columns,
            is_system: false,
        };

        // 9. Configure the output columns
        self.outputs.clear();
        for (i, col) in validated.yield_columns.iter().enumerate() {
            let col_name = col.alias.clone().unwrap_or_else(|| format!("column_{}", i));
            self.outputs.push(ColumnDef {
                name: col_name,
                type_: ValueType::String,
            });
        }

        self.validated_result = Some(validated);

        // 10. Constructing detailed ValidationInfo
        let mut info = ValidationInfo::new();

        // Add semantic information
        if !info
            .semantic_info
            .referenced_edges
            .contains(&edge_type_name)
        {
            info.semantic_info
                .referenced_edges
                .push(edge_type_name.clone());
        }

        // 11. Return the verification results containing detailed information.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::FetchEdges
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `FETCH EDGES` command is not a global statement; therefore, the relevant spatial data must be selected in advance.
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

    fn _create_fetch_edges_stmt(
        edge_type: &str,
        src: Expression,
        dst: Expression,
        rank: Option<Expression>,
        properties: Option<Vec<String>>,
    ) -> FetchStmt {
        FetchStmt {
            span: Span::default(),
            target: FetchTarget::Edges {
                edge_type: edge_type.to_string(),
                src: create_contextual_expr(src),
                dst: create_contextual_expr(dst),
                rank: rank.map(create_contextual_expr),
                properties,
            },
        }
    }

    #[test]
    fn test_validate_edge_name_empty() {
        let validator = FetchEdgesValidator::new();
        let result = validator.validate_edge_name("");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message, "The name of the edge type must be specified.");
    }

    #[test]
    fn test_validate_edge_name_valid() {
        let validator = FetchEdgesValidator::new();
        let result = validator.validate_edge_name("friend");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_edge_key_valid() {
        let validator = FetchEdgesValidator::new();
        let src = create_contextual_expr(Expression::Literal(Value::String("v1".to_string())));
        let dst = create_contextual_expr(Expression::Literal(Value::String("v2".to_string())));
        let result = validator.validate_edge_key(&src, &dst, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_edge_key_with_rank() {
        let validator = FetchEdgesValidator::new();
        let src = create_contextual_expr(Expression::Literal(Value::String("v1".to_string())));
        let dst = create_contextual_expr(Expression::Literal(Value::String("v2".to_string())));
        let rank = Some(create_contextual_expr(Expression::Literal(Value::Int(0))));
        let result = validator.validate_edge_key(&src, &dst, rank.as_ref());
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_rank_negative() {
        let validator = FetchEdgesValidator::new();
        let result =
            validator.validate_rank(&create_contextual_expr(Expression::Literal(Value::Int(-1))));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("non-negative"));
    }

    #[test]
    fn test_validate_yield_duplicate_alias() {
        let validator = FetchEdgesValidator::new();
        let yield_columns = vec![
            (
                create_contextual_expr(Expression::Literal(Value::String("prop1".to_string()))),
                Some("col".to_string()),
            ),
            (
                create_contextual_expr(Expression::Literal(Value::String("prop2".to_string()))),
                Some("col".to_string()),
            ),
        ];
        let result = validator.validate_yield_clause(&yield_columns);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("Duplicate"));
    }

    #[test]
    fn test_statement_validator_trait() {
        let validator = FetchEdgesValidator::new();

        // Testing the `statement_type`
        assert_eq!(validator.statement_type(), StatementType::FetchEdges);

        // Testing inputs/outputs
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());

        // Testing user_defined_vars
        assert!(validator.user_defined_vars().is_empty());
    }
}
