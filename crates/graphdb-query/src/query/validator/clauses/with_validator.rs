//! With the statement validator…
//! Used to validate WITH statements (Cypher-style pipeline clauses)

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::{Ast, ReturnItem, WithStmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::helpers::schema_validator::SchemaValidator;
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::collections::HashMap;
use std::sync::Arc;

/// With the statement validator
#[derive(Debug)]
pub struct WithValidator {
    items: Vec<ReturnItem>,
    where_clause: Option<ContextualExpression>,
    distinct: bool,
    order_by: Option<crate::query::parser::ast::stmt::OrderByClause>,
    skip: Option<usize>,
    limit: Option<usize>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
    // Schema validator for property validation
    schema_validator: Option<SchemaValidator>,
    // Space name for schema lookup
    space_name: Option<String>,
    // Available variables and their types (variable_name -> tag_name/edge_type)
    available_vars: HashMap<String, String>,
}

impl WithValidator {
    /// Create a new With validator.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            where_clause: None,
            distinct: false,
            order_by: None,
            skip: None,
            limit: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            schema_validator: None,
            space_name: None,
            available_vars: HashMap::new(),
        }
    }

    /// Create a new instance with schema manager
    pub fn with_schema_manager(schema_manager: Arc<SchemaManager>) -> Self {
        Self {
            items: Vec::new(),
            where_clause: None,
            distinct: false,
            order_by: None,
            skip: None,
            limit: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            schema_validator: Some(SchemaValidator::new(schema_manager)),
            space_name: None,
            available_vars: HashMap::new(),
        }
    }

    /// Set schema manager
    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_validator = Some(SchemaValidator::new(schema_manager));
    }

    /// Set space name
    pub fn set_space_name(&mut self, space_name: String) {
        self.space_name = Some(space_name);
    }

    /// Set available variables and their types
    pub fn set_available_vars(&mut self, vars: HashMap<String, String>) {
        self.available_vars = vars;
    }

    /// Add a variable with its type
    pub fn add_available_var(&mut self, var_name: String, var_type: String) {
        self.available_vars.insert(var_name, var_type);
    }

    /// Verify the returned items.
    fn validate_return_item(&self, item: &ReturnItem) -> Result<ColumnDef, ValidationError> {
        match item {
            ReturnItem::Expression { expression, alias } => {
                // Verify the expression
                self.validate_expression(expression)?;

                //  Determine the column names.
                let name = alias
                    .clone()
                    .or_else(|| self.infer_column_name(expression))
                    .unwrap_or_else(|| "column".to_string());

                // Inference type
                let type_ = self.infer_expression_type(expression);

                Ok(ColumnDef { name, type_ })
            }
        }
    }

    /// Verify the expression
    fn validate_expression(&self, expr: &ContextualExpression) -> Result<(), ValidationError> {
        if let Some(e) = expr.get_expression() {
            self.validate_expression_internal(&e)
        } else {
            Err(ValidationError::new(
                "Invalid expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Internal method: Verifying expressions
    fn validate_expression_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Literal(_) => Ok(()),
            Expression::Variable(var) => {
                // Check whether the variable comes from the input.
                if !self.inputs.iter().any(|c| &c.name == var)
                    && !self.user_defined_vars.iter().any(|v| v == var)
                {
                    return Err(ValidationError::new(
                        format!("Variable '{}' not available in WITH clause", var),
                        ValidationErrorType::SemanticError,
                    ));
                }
                Ok(())
            }
            Expression::Property { object, property } => {
                self.validate_expression_internal(object)?;
                if property.is_empty() {
                    return Err(ValidationError::new(
                        "Property name cannot be empty".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                // Validate property reference if schema is available
                if let (Some(ref schema_validator), Some(ref space_name)) =
                    (&self.schema_validator, &self.space_name)
                {
                    if let Err(e) = schema_validator.validate_expression_properties(
                        expr,
                        space_name,
                        &self.available_vars,
                    ) {
                        return Err(ValidationError::new(
                            e.message,
                            ValidationErrorType::SemanticError,
                        ));
                    }
                }
                Ok(())
            }
            Expression::Function { name, args } => self.validate_function_call_internal(name, args),
            Expression::Binary { left, right, .. } => {
                self.validate_expression_internal(left)?;
                self.validate_expression_internal(right)
            }
            Expression::Unary { operand, .. } => self.validate_expression_internal(operand),
            _ => Ok(()),
        }
    }

    /// Internal method: Verification of function calls
    fn validate_function_call_internal(
        &self,
        name: &str,
        args: &[crate::core::types::expr::Expression],
    ) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(
                "Function name cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        for arg in args {
            self.validate_expression_internal(arg)?;
        }

        Ok(())
    }

    /// Verify the WHERE clause
    fn validate_where_clause(
        &self,
        where_clause: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        self.validate_expression(where_clause)?;

        // The WHERE clause must be of a boolean type or can be converted to a boolean type.
        if let Some(e) = where_clause.get_expression() {
            use crate::core::types::expr::Expression;
            match e {
                Expression::Literal(_)
                | Expression::Variable(_)
                | Expression::Binary { .. }
                | Expression::Unary { .. }
                | Expression::Function { .. } => Ok(()),
                _ => Err(ValidationError::new(
                    "WHERE clause must be a boolean expression".to_string(),
                    ValidationErrorType::TypeError,
                )),
            }
        } else {
            Err(ValidationError::new(
                "WHERE expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Infer the column names
    fn infer_column_name(&self, expr: &ContextualExpression) -> Option<String> {
        if let Some(e) = expr.get_expression() {
            self.infer_column_name_internal(&e)
        } else {
            None
        }
    }

    /// Internal method: Inferring column names
    fn infer_column_name_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Option<String> {
        use crate::core::types::expr::Expression;

        match expr {
            Expression::Variable(name) => Some(name.clone()),
            Expression::Property { property, .. } => Some(property.clone()),
            Expression::Function { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// Determine the type of the expression
    fn infer_expression_type(&self, expr: &ContextualExpression) -> ValueType {
        if let Some(e) = expr.get_expression() {
            self.infer_expression_type_internal(&e)
        } else {
            ValueType::Unknown
        }
    }

    /// Internal method: Inferring the type of an expression
    fn infer_expression_type_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> ValueType {
        // If schema validator is available, use it for type inference
        if let (Some(ref schema_validator), Some(ref space_name)) =
            (&self.schema_validator, &self.space_name)
        {
            let input_columns: HashMap<String, ValueType> = self
                .inputs
                .iter()
                .map(|c| (c.name.clone(), c.type_.clone()))
                .collect();

            return schema_validator.infer_expression_type(
                expr,
                space_name,
                &self.available_vars,
                &input_columns,
            );
        }

        // Fallback to basic type inference
        use crate::core::types::expr::Expression;
        use crate::core::Value;

        match expr {
            Expression::Literal(value) => match value {
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
            },
            Expression::Variable(name) => {
                // Look up in input columns
                for input in &self.inputs {
                    if &input.name == name {
                        return input.type_.clone();
                    }
                }
                ValueType::Unknown
            }
            Expression::List(_) => ValueType::List,
            Expression::Map(_) => ValueType::Map,
            _ => ValueType::Unknown,
        }
    }

    /// Verify the ORDER BY clause
    fn validate_order_by(
        &self,
        order_by: &crate::query::parser::ast::stmt::OrderByClause,
    ) -> Result<(), ValidationError> {
        for item in &order_by.items {
            self.validate_expression(&item.expression)?;
        }
        Ok(())
    }

    /// Verify SKIP and LIMIT
    fn validate_skip_limit(
        &self,
        skip: Option<usize>,
        limit: Option<usize>,
    ) -> Result<(), ValidationError> {
        if let Some(s) = skip {
            if s > 1_000_000 {
                return Err(ValidationError::new(
                    format!("SKIP value {} exceeds maximum allowed (1000000)", s),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        if let Some(l) = limit {
            if l > 1_000_000 {
                return Err(ValidationError::new(
                    format!("LIMIT value {} exceeds maximum allowed (1000000)", l),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(())
    }

    fn validate_impl(&mut self, stmt: &WithStmt) -> Result<(), ValidationError> {
        // Verify the returned items.
        if stmt.items.is_empty() {
            return Err(ValidationError::new(
                "WITH clause must have at least one item".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        for item in &stmt.items {
            let col = self.validate_return_item(item)?;
            self.outputs.push(col);
        }

        // Verify the WHERE clause
        if let Some(ref where_clause) = stmt.where_clause {
            self.validate_where_clause(where_clause)?;
        }

        // Verify the ORDER BY clause.
        if let Some(ref order_by) = stmt.order_by {
            self.validate_order_by(order_by)?;
        }

        // Verify SKIP and LIMIT
        self.validate_skip_limit(stmt.skip, stmt.limit)?;

        // Save the information.
        self.items = stmt.items.clone();
        self.where_clause = stmt.where_clause.clone();
        self.distinct = stmt.distinct;
        self.order_by = stmt.order_by.clone();
        self.skip = stmt.skip;
        self.limit = stmt.limit;

        // Update the user-defined variable to the output column.
        self.user_defined_vars = self.outputs.iter().map(|c| c.name.clone()).collect();

        Ok(())
    }

    /// Setting the input column (the column that is passed from the upstream source)
    pub fn set_inputs(&mut self, inputs: Vec<ColumnDef>) {
        // Initially, the user-defined variables come from the input.
        self.user_defined_vars = inputs.iter().map(|c| c.name.clone()).collect();
        self.inputs = inputs;
    }
}

impl Default for WithValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for WithValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        // Get space information from QueryContext
        if let Some(space_name) = qctx.space_name() {
            self.space_name = Some(space_name);
        }

        let with_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::With(with_stmt) => with_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected WITH statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(with_stmt)?;

        let mut info = ValidationInfo::new();

        for item in &self.items {
            match item {
                ReturnItem::Expression { expression, alias } => {
                    if let Some(ref alias_name) = alias {
                        info.add_alias(alias_name.clone(), AliasType::CTE);
                    }
                    info.semantic_info
                        .output_fields
                        .push(format!("{:?}", expression));
                }
            }
        }

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::With
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // “WITH” is not a global statement.
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
    use crate::core::Value;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;

    #[test]
    fn test_with_validator_new() {
        let validator = WithValidator::new();
        assert_eq!(validator.statement_type(), StatementType::With);
        assert!(!validator.is_global_statement());
    }

    #[test]
    fn test_validate_where_clause() {
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use std::sync::Arc;

        let validator = WithValidator::new();

        // A valid WHERE clause
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Literal(Value::Bool(true));
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        let where_expr = ContextualExpression::new(id, expr_ctx);
        assert!(validator.validate_where_clause(&where_expr).is_ok());

        // Binary operator
        let _expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let _expr = Expression::Binary {
            left: Box::new(Expression::Variable("n".to_string())),
            op: crate::core::types::operators::BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::Int(1))),
        };
        let _meta = ExpressionMeta::new(_expr);
        let _id = _expr_ctx.register_expression(_meta);
        let _where_expr = ContextualExpression::new(_id, _expr_ctx);
        // This will fail because the variable `n` is not included in the input data.
        // assert!(validator.validate_where_clause(&_where_expr).is_err());
    }

    #[test]
    fn test_validate_skip_limit() {
        let validator = WithValidator::new();

        // Valid values
        assert!(validator.validate_skip_limit(Some(10), Some(100)).is_ok());

        // Exceeds the maximum value.
        assert!(validator
            .validate_skip_limit(Some(2_000_000), None)
            .is_err());
    }
}
