//! LIMIT clause validator
//! Verify the expressions of the LIMIT and SKIP clauses.

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::Ast;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Verified LIMIT information
#[derive(Debug, Clone)]
pub struct ValidatedLimit {
    pub space_id: u64,
    pub skip: Option<u64>,
    pub limit: Option<u64>,
    pub count: Option<u64>,
}

#[derive(Debug)]
pub struct LimitValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expression_props: ExpressionProps,
    user_defined_vars: Vec<String>,
    validated_result: Option<ValidatedLimit>,
    schema_manager: Option<Arc<SchemaManager>>,
    skip_expr: Option<ContextualExpression>,
    limit_expr: Option<ContextualExpression>,
    count: Option<u64>,
}

impl LimitValidator {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            expression_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validated_result: None,
            schema_manager: None,
            skip_expr: None,
            limit_expr: None,
            count: None,
        }
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    pub fn set_skip(mut self, skip: ContextualExpression) -> Self {
        self.skip_expr = Some(skip);
        self
    }

    pub fn set_limit(mut self, limit: ContextualExpression) -> Self {
        self.limit_expr = Some(limit);
        self
    }

    pub fn set_count(mut self, count: u64) -> Self {
        self.count = Some(count);
        self
    }

    /// Verify the SKIP expression
    fn validate_skip(
        &self,
        skip: &Option<ContextualExpression>,
    ) -> Result<Option<u64>, ValidationError> {
        if let Some(skip_expr) = skip {
            // Verify whether the type is an integer.
            if !self.is_integer_expression(skip_expr) {
                return Err(ValidationError::new(
                    "SKIP value must be integer type".to_string(),
                    ValidationErrorType::TypeError,
                ));
            }

            // Evaluating an expression
            let skip_val = self.evaluate_expression(skip_expr)?;
            if skip_val < 0 {
                return Err(ValidationError::new(
                    "SKIP value cannot be negative".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
            Ok(Some(skip_val as u64))
        } else {
            Ok(None)
        }
    }

    /// Verify the LIMIT expression
    fn validate_limit(
        &self,
        limit: &Option<ContextualExpression>,
    ) -> Result<Option<u64>, ValidationError> {
        if let Some(limit_expr) = limit {
            // Verify whether the type is an integer.
            if !self.is_integer_expression(limit_expr) {
                return Err(ValidationError::new(
                    "LIMIT value must be integer type".to_string(),
                    ValidationErrorType::TypeError,
                ));
            }

            // Evaluating an expression
            let limit_val = self.evaluate_expression(limit_expr)?;
            if limit_val < 0 {
                return Err(ValidationError::new(
                    "LIMIT value cannot be negative".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
            Ok(Some(limit_val as u64))
        } else {
            Ok(None)
        }
    }

    /// Verification scope
    fn validate_range(&self, skip: Option<u64>, limit: Option<u64>) -> Result<(), ValidationError> {
        let skip_val = skip.unwrap_or(0);
        let limit_val = limit.unwrap_or(0);

        if skip_val == 0 && limit_val == 0 {
            return Err(ValidationError::new(
                "At least one of SKIP or LIMIT must be greater than zero".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        Ok(())
    }

    /// Verify the count.
    fn validate_count(&self, count: Option<u64>) -> Result<(), ValidationError> {
        if let Some(c) = count {
            if c > u64::MAX / 2 {
                return Err(ValidationError::new(
                    "LIMIT value is too large".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Check whether the expression is of the integer type.
    fn is_integer_expression(&self, expr: &ContextualExpression) -> bool {
        if let Some(e) = expr.get_expression() {
            self.is_integer_expression_internal(&e)
        } else {
            false
        }
    }

    /// Internal method: Checks whether the expression is of the integer type.
    fn is_integer_expression_internal(&self, expr: &crate::core::types::expr::Expression) -> bool {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Literal(val) => matches!(
                val,
                crate::core::Value::SmallInt(_)
                    | crate::core::Value::Int(_)
                    | crate::core::Value::BigInt(_)
            ),
            Expression::Variable(_) => true, // Variables are checked during runtime.
            _ => false,
        }
    }

    /// Evaluating an expression
    fn evaluate_expression(&self, expr: &ContextualExpression) -> Result<i64, ValidationError> {
        if let Some(e) = expr.get_expression() {
            self.evaluate_expression_internal(&e)
        } else {
            Err(ValidationError::new(
                "Unable to evaluate expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Internal method: Evaluating expressions
    fn evaluate_expression_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<i64, ValidationError> {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Literal(crate::core::Value::SmallInt(n)) => Ok(*n as i64),
            Expression::Literal(crate::core::Value::Int(n)) => Ok(*n as i64),
            Expression::Literal(crate::core::Value::BigInt(n)) => Ok(*n),
            Expression::Variable(_) => Ok(0), // Variables are parsed at runtime.
            _ => Err(ValidationError::new(
                "Cannot evaluate expression".to_string(),
                ValidationErrorType::SemanticError,
            )),
        }
    }

    /// Generate a column of outputs.
    fn generate_output_columns(&mut self) {
        self.outputs.clear();
        self.outputs.push(ColumnDef {
            name: "LIMIT_RESULT".to_string(),
            type_: ValueType::List,
        });
    }
}

impl Default for LimitValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for LimitValidator {
    fn validate(
        &mut self,
        _ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // 1. Check whether additional space is needed.
        if !self.is_global_statement() && qctx.space_id().is_none() {
            return Err(ValidationError::new(
                "No image space selected, please execute first USE <space>".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // 2. Retrieve the LIMIT statement (if it exists).
        // The QueryStmt does not contain the skip/limit fields; therefore, the default values set by the validator are used.
        let (skip_opt, limit_opt) = (self.skip_expr.clone(), self.limit_expr.clone());

        // 3. Verify SKIP
        let skip_val = self.validate_skip(&skip_opt)?;

        // 4. Verify the LIMIT
        let limit_val = self.validate_limit(&limit_opt)?;

        // 5. Scope of verification
        self.validate_range(skip_val, limit_val)?;

        // 6. Verify the count
        self.validate_count(self.count)?;

        // 7. Obtain the space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 8. Create the validation results
        let validated = ValidatedLimit {
            space_id,
            skip: skip_val,
            limit: limit_val,
            count: self.count,
        };

        self.validated_result = Some(validated);

        // 9. Generate an output column.
        self.generate_output_columns();

        // 10. Constructing ValidationInfo
        let mut info = ValidationInfo::new();

        if let Some(skip) = skip_val {
            info.semantic_info.pagination_offset = Some(skip as usize);
        }

        if let Some(limit) = limit_val {
            info.semantic_info.pagination_limit = Some(limit as usize);
        }

        // 11. Return the verification results.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Limit
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `LIMIT` statement is not a global statement; it is necessary to select a specific area (or “space”) in advance before using it.
        false
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expression_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::core::types::expr::Expression;
    use crate::core::Value;
    use crate::query::parser::ast::Stmt;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use crate::query::QueryRequestContext;

    /// Create a QueryContext for testing purposes, which should contain a valid space_id.
    fn create_test_query_context() -> Arc<QueryContext> {
        let rctx = Arc::new(QueryRequestContext::new("TEST".to_string()));
        let mut qctx = QueryContext::new(rctx);
        let space_info = crate::core::types::SpaceInfo::new("test_space".to_string());
        qctx.set_space_info(space_info);
        Arc::new(qctx)
    }

    fn create_test_ast(stmt: Stmt) -> Arc<Ast> {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        Arc::new(Ast::new(stmt, ctx))
    }

    #[test]
    fn test_limit_validator_basic() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let limit_expr = Expression::literal(10);
        let meta = crate::core::types::expr::ExpressionMeta::new(limit_expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);

        let mut validator = LimitValidator::new().set_limit(ctx_expr);

        let qctx = create_test_query_context();
        let use_stmt = crate::query::parser::ast::UseStmt {
            span: crate::core::types::Span::default(),
            space: "test".to_string(),
        };
        let result = validator.validate(create_test_ast(Stmt::Use(use_stmt)), qctx);
        assert!(result.is_ok());

        let validated = validator
            .validated_result
            .expect("Failed to get validated result");
        assert_eq!(validated.limit, Some(10));
    }

    #[test]
    fn test_limit_validator_with_skip() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let skip_expr = Expression::literal(5);
        let skip_meta = crate::core::types::expr::ExpressionMeta::new(skip_expr);
        let skip_id = expr_ctx.register_expression(skip_meta);
        let skip_ctx_expr = ContextualExpression::new(skip_id, expr_ctx.clone());

        let limit_expr = Expression::literal(10);
        let limit_meta = crate::core::types::expr::ExpressionMeta::new(limit_expr);
        let limit_id = expr_ctx.register_expression(limit_meta);
        let limit_ctx_expr = ContextualExpression::new(limit_id, expr_ctx);

        let mut validator = LimitValidator::new()
            .set_skip(skip_ctx_expr)
            .set_limit(limit_ctx_expr);

        let qctx = create_test_query_context();
        let use_stmt = crate::query::parser::ast::UseStmt {
            span: crate::core::types::Span::default(),
            space: "test".to_string(),
        };
        let result = validator.validate(create_test_ast(Stmt::Use(use_stmt)), qctx);
        assert!(result.is_ok());

        let validated = validator
            .validated_result
            .expect("Failed to get validated result");
        assert_eq!(validated.skip, Some(5));
        assert_eq!(validated.limit, Some(10));
    }

    #[test]
    fn test_limit_validator_negative_skip() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let skip_expr = Expression::literal(-1);
        let meta = crate::core::types::expr::ExpressionMeta::new(skip_expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);

        let mut validator = LimitValidator::new().set_skip(ctx_expr);

        let qctx = create_test_query_context();
        let use_stmt = crate::query::parser::ast::UseStmt {
            span: crate::core::types::Span::default(),
            space: "test".to_string(),
        };
        let result = validator.validate(create_test_ast(Stmt::Use(use_stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("cannot be negative"));
    }

    #[test]
    fn test_limit_validator_negative_limit() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let limit_expr = Expression::literal(-5);
        let meta = crate::core::types::expr::ExpressionMeta::new(limit_expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);

        let mut validator = LimitValidator::new().set_limit(ctx_expr);

        let qctx = create_test_query_context();
        let use_stmt = crate::query::parser::ast::UseStmt {
            span: crate::core::types::Span::default(),
            space: "test".to_string(),
        };
        let result = validator.validate(create_test_ast(Stmt::Use(use_stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("cannot be negative"));
    }

    #[test]
    fn test_limit_validator_zero_skip_and_limit() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let skip_expr = Expression::literal(0);
        let skip_meta = crate::core::types::expr::ExpressionMeta::new(skip_expr);
        let skip_id = expr_ctx.register_expression(skip_meta);
        let skip_ctx_expr = ContextualExpression::new(skip_id, expr_ctx.clone());

        let limit_expr = Expression::literal(0);
        let limit_meta = crate::core::types::expr::ExpressionMeta::new(limit_expr);
        let limit_id = expr_ctx.register_expression(limit_meta);
        let limit_ctx_expr = ContextualExpression::new(limit_id, expr_ctx);

        let mut validator = LimitValidator::new()
            .set_skip(skip_ctx_expr)
            .set_limit(limit_ctx_expr);

        let qctx = create_test_query_context();
        let use_stmt = crate::query::parser::ast::UseStmt {
            span: crate::core::types::Span::default(),
            space: "test".to_string(),
        };
        let result = validator.validate(create_test_ast(Stmt::Use(use_stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("greater than zero"));
    }

    #[test]
    fn test_limit_validator_non_integer() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let limit_expr = Expression::literal("invalid");
        let meta = crate::core::types::expr::ExpressionMeta::new(limit_expr);
        let id = expr_ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, expr_ctx);

        let mut validator = LimitValidator::new().set_limit(ctx_expr);

        let qctx = create_test_query_context();
        let use_stmt = crate::query::parser::ast::UseStmt {
            span: crate::core::types::Span::default(),
            space: "test".to_string(),
        };
        let result = validator.validate(create_test_ast(Stmt::Use(use_stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("must be integer"));
    }

    #[test]
    fn test_limit_validator_trait_interface() {
        let validator = LimitValidator::new();

        assert_eq!(validator.statement_type(), StatementType::Limit);
        assert!(validator.inputs().is_empty());
        assert!(validator.user_defined_vars().is_empty());
    }

    #[test]
    fn test_limit_validator_count() {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let limit_expr = Expression::Literal(Value::Int(10));
        let meta = crate::core::types::expr::ExpressionMeta::new(limit_expr);
        let id = expr_ctx.register_expression(meta);
        let limit_ctx_expr = ContextualExpression::new(id, expr_ctx);

        let mut validator = LimitValidator::new()
            .set_limit(limit_ctx_expr)
            .set_count(100);

        let qctx = create_test_query_context();
        let use_stmt = crate::query::parser::ast::UseStmt {
            span: crate::core::types::Span::default(),
            space: "test".to_string(),
        };
        let result = validator.validate(create_test_ast(Stmt::Use(use_stmt)), qctx);
        assert!(result.is_ok());

        let validated = validator
            .validated_result
            .expect("Failed to get validated result");
        assert_eq!(validated.count, Some(100));
    }
}
