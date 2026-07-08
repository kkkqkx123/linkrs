//! GroupBy statement validator
//! Corresponds to the functionality of the NebulaGraph GroupByValidator
//! Verify the validity of the GROUP BY statement.
//!
//! Design principles:
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. Verify the validity of the group key and the aggregate expression.
//! 3. Support for verifying HAVING clauses

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Expression;
use crate::query::parser::ast::stmt::{Ast, GroupByStmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;
use std::sync::Arc;

/// Verified GroupBy information
#[derive(Debug, Clone)]
pub struct ValidatedGroupBy {
    pub group_keys: Vec<ContextualExpression>,
    pub group_items: Vec<ContextualExpression>,
    pub output_col_names: Vec<String>,
    pub need_gen_project: bool,
}

/// GroupBy Validator
#[derive(Debug)]
pub struct GroupByValidator {
    group_keys: Vec<ContextualExpression>,
    group_items: Vec<ContextualExpression>,
    agg_output_col_names: Vec<String>,
    need_gen_project: bool,
    yield_cols: Vec<ContextualExpression>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
}

impl GroupByValidator {
    pub fn new() -> Self {
        Self {
            group_keys: Vec::new(),
            group_items: Vec::new(),
            agg_output_col_names: Vec::new(),
            need_gen_project: false,
            yield_cols: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            expr_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
        }
    }

    fn validate_impl(&mut self, stmt: &GroupByStmt) -> Result<(), ValidationError> {
        // Verify the group key
        self.validate_group_keys(&stmt.group_items)?;

        // Verify the YIELD clause
        self.validate_yield(&stmt.yield_clause)?;

        // Semantic checking
        self.group_clause_semantic_check()?;

        // Verify the HAVING clause
        if let Some(ref having) = stmt.having_clause {
            self.validate_having(having)?;
        }

        self.setup_outputs();
        Ok(())
    }

    fn validate_group_keys(
        &mut self,
        group_items: &[ContextualExpression],
    ) -> Result<(), ValidationError> {
        if group_items.is_empty() {
            return Err(ValidationError::new(
                "GROUP BY clause must have at least one key".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        for item in group_items {
            // The validation group key must be a valid expression.
            self.validate_group_key(item)?;
            self.group_keys.push(item.clone());
        }

        Ok(())
    }

    fn validate_group_key(&self, expr: &ContextualExpression) -> Result<(), ValidationError> {
        if let Some(e) = expr.get_expression() {
            self.validate_group_key_internal(&e)
        } else {
            Err(ValidationError::new(
                "Invalid grouping key expression".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Internal method: Validate the group key
    fn validate_group_key_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        // The grouping key can be:
        // 1. List of references
        // 2. Attribute Access
        // 3. Simple expressions
        match expr {
            Expression::Variable(_) | Expression::Property { .. } => Ok(()),
            Expression::Function { name, .. } => {
                // Aggregation functions are not allowed in the grouping key.
                if Self::is_aggregate_function(name) {
                    Err(ValidationError::new(
                        format!("Aggregate function {} cannot be used in GROUP BY key", name),
                        ValidationErrorType::SemanticError,
                    ))
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }

    fn validate_yield(
        &mut self,
        yield_clause: &crate::query::parser::ast::stmt::YieldClause,
    ) -> Result<(), ValidationError> {
        for item in &yield_clause.items {
            let expr = &item.expression;

            // Check the aggregate functions in the expression.
            if Self::contains_aggregate(expr) {
                self.agg_output_col_names.push(
                    item.alias
                        .clone()
                        .unwrap_or_else(|| Self::expr_to_string(expr)),
                );
            }

            self.group_items.push(expr.clone());

            // Saving the `yield` column is used for semantic checking.
            self.yield_cols.push(expr.clone());
        }

        Ok(())
    }

    fn validate_having(&self, having: &ContextualExpression) -> Result<(), ValidationError> {
        // The expression in the HAVING clause must be a valid Boolean expression.
        // And aggregate functions can also be included.
        if let Some(e) = having.get_expression() {
            self.validate_having_expr_internal(&e)
        } else {
            Err(ValidationError::new(
                "HAVING expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ))
        }
    }

    /// Internal method: Verifying the HAVING expression
    fn validate_having_expr_internal(
        &self,
        expr: &crate::core::types::expr::Expression,
    ) -> Result<(), ValidationError> {
        match expr {
            Expression::Binary { op, left, right } => {
                self.validate_having_expr_internal(left)?;
                self.validate_having_expr_internal(right)?;

                // Verification of comparison operators
                match op {
                    crate::core::types::operators::BinaryOperator::Equal
                    | crate::core::types::operators::BinaryOperator::NotEqual
                    | crate::core::types::operators::BinaryOperator::LessThan
                    | crate::core::types::operators::BinaryOperator::GreaterThan
                    | crate::core::types::operators::BinaryOperator::LessThanOrEqual
                    | crate::core::types::operators::BinaryOperator::GreaterThanOrEqual
                    | crate::core::types::operators::BinaryOperator::And
                    | crate::core::types::operators::BinaryOperator::Or => Ok(()),
                    _ => Err(ValidationError::new(
                        format!("Invalid operator in HAVING clause: {:?}", op),
                        ValidationErrorType::SemanticError,
                    )),
                }
            }
            Expression::Unary { op, operand } => {
                self.validate_having_expr_internal(operand)?;
                match op {
                    crate::core::types::operators::UnaryOperator::Not => Ok(()),
                    _ => Err(ValidationError::new(
                        format!("Invalid unary operator in HAVING clause: {:?}", op),
                        ValidationErrorType::SemanticError,
                    )),
                }
            }
            _ => Ok(()),
        }
    }

    fn group_clause_semantic_check(&self) -> Result<(), ValidationError> {
        // Check whether all the non-aggregated expressions in the YIELD clause are also included in the GROUP BY clause.
        for yield_col in &self.yield_cols {
            if !Self::contains_aggregate(yield_col) {
                // Non-aggregate expressions must be included in the GROUP BY clause.
                let found = self
                    .group_keys
                    .iter()
                    .any(|key| Self::expr_equivalent(key, yield_col));

                if !found {
                    return Err(ValidationError::new(
                        format!(
                            "Expression '{}' must appear in GROUP BY clause or be used in an aggregate function",
                            Self::expr_to_string(yield_col)
                        ),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }

        Ok(())
    }

    fn setup_outputs(&mut self) {
        // The output column is set according to the settings specified in the YIELD clause.
        self.outputs = self
            .group_items
            .iter()
            .zip(&self.agg_output_col_names)
            .map(|(_, name)| ColumnDef {
                name: name.clone(),
                type_: ValueType::Unknown,
            })
            .collect();
    }

    fn is_aggregate_function(name: &str) -> bool {
        matches!(
            name.to_uppercase().as_str(),
            "COUNT" | "SUM" | "AVG" | "MAX" | "MIN" | "COLLECT" | "STDDEV"
        )
    }

    fn contains_aggregate(expr: &ContextualExpression) -> bool {
        if let Some(e) = expr.get_expression() {
            Self::contains_aggregate_internal(&e)
        } else {
            false
        }
    }

    /// Internal method: Check whether the expression contains aggregate functions.
    fn contains_aggregate_internal(expr: &crate::core::types::expr::Expression) -> bool {
        match expr {
            Expression::Function { name, .. } => {
                if Self::is_aggregate_function(name) {
                    return true;
                }
                false
            }
            Expression::Binary { left, right, .. } => {
                Self::contains_aggregate_internal(left) || Self::contains_aggregate_internal(right)
            }
            Expression::Unary { operand, .. } => Self::contains_aggregate_internal(operand),
            _ => false,
        }
    }

    fn expr_equivalent(a: &ContextualExpression, b: &ContextualExpression) -> bool {
        // Simplified implementation: Comparing string representations
        Self::expr_to_string(a) == Self::expr_to_string(b)
    }

    fn expr_to_string(expr: &ContextualExpression) -> String {
        if let Some(e) = expr.get_expression() {
            format!("{:?}", e)
        } else {
            "InvalidExpression".to_string()
        }
    }

    pub fn validated_result(&self) -> ValidatedGroupBy {
        ValidatedGroupBy {
            group_keys: self.group_keys.clone(),
            group_items: self.group_items.clone(),
            output_col_names: self.agg_output_col_names.clone(),
            need_gen_project: self.need_gen_project,
        }
    }

    pub fn group_keys(&self) -> &[ContextualExpression] {
        &self.group_keys
    }

    pub fn group_items(&self) -> &[ContextualExpression] {
        &self.group_items
    }

    pub fn need_gen_project(&self) -> bool {
        self.need_gen_project
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as parameters.
impl StatementValidator for GroupByValidator {
    fn validate(
        &mut self,
        ast: Arc<Ast>,
        _qctx: Arc<QueryContext>,
    ) -> Result<ValidationResult, ValidationError> {
        let group_by_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::GroupBy(group_by_stmt) => group_by_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected GROUP BY statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        self.validate_impl(group_by_stmt)?;

        let info = ValidationInfo::new();

        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::GroupBy
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        false
    }

    fn expression_props(&self) -> &ExpressionProps {
        &self.expr_props
    }

    fn user_defined_vars(&self) -> &[String] {
        &self.user_defined_vars
    }
}

impl Default for GroupByValidator {
    fn default() -> Self {
        Self::new()
    }
}
