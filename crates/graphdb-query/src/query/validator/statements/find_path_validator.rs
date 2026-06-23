//! FIND PATH Statement Validator – New System Version
//! Verify the validity of the FIND PATH statement.

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::{Ast, FindPathStmt};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified path search information
#[derive(Debug, Clone)]
pub struct ValidatedFindPath {
    pub space_id: u64,
    pub from: crate::query::parser::ast::stmt::FromClause,
    pub to: ContextualExpression,
    pub over: Option<crate::query::parser::ast::stmt::OverClause>,
    pub where_clause: Option<ContextualExpression>,
    pub shortest: bool,
    pub max_steps: Option<usize>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub yield_clause: Option<crate::query::parser::ast::stmt::YieldClause>,
    pub weight_expression: Option<String>,
    pub heuristic_expression: Option<String>,
    pub with_loop: bool,
    pub with_cycle: bool,
}

/// FIND PATH Validator – New System Implementation
#[derive(Debug)]
pub struct FindPathValidator {
    schema_manager: Option<Arc<SchemaManager>>,
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expr_props: ExpressionProps,
    user_defined_vars: Vec<String>,
    validated_result: Option<ValidatedFindPath>,
}

impl FindPathValidator {
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

    pub fn validated_result(&self) -> Option<&ValidatedFindPath> {
        self.validated_result.as_ref()
    }

    fn validate_find_path(&self, stmt: &FindPathStmt) -> Result<(), ValidationError> {
        // Verify the FROM clause
        if stmt.from.vertices.is_empty() {
            return Err(ValidationError::new(
                "FIND PATH must specify source vertices in FROM clause".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Verification of the step count limit
        if let Some(max_steps) = stmt.max_steps {
            if max_steps > 100 {
                return Err(ValidationError::new(
                    "Maximum steps cannot exceed 100".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }

        Ok(())
    }

    fn validate_yield_clause(
        &self,
        yield_clause: &Option<crate::query::parser::ast::stmt::YieldClause>,
    ) -> Result<(), ValidationError> {
        if let Some(ref yc) = yield_clause {
            let mut seen_names: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for item in &yc.items {
                let name = item
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", item.expression));
                let count = seen_names.entry(name.clone()).or_insert(0);
                *count += 1;
                if *count > 1 {
                    return Err(ValidationError::new(
                        format!("Duplicate column name '{}' in YIELD clause", name),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Default for FindPathValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for FindPathValidator {
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

        // 2. Obtain the FIND PATH statement (with ownership rights).
        let find_path_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::FindPath(s) => s,
            _ => {
                return Err(ValidationError::new(
                    "Expected FIND PATH statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 3. Perform basic validation.
        self.validate_find_path(find_path_stmt)?;

        // 4. Verify the YIELD clause
        self.validate_yield_clause(&find_path_stmt.yield_clause)?;

        // 5. Obtain the space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 6. Create verification results (transfer ownership directly, without the need to clone).
        let validated = ValidatedFindPath {
            space_id,
            from: find_path_stmt.from.clone(),
            to: find_path_stmt.to.clone(),
            over: find_path_stmt.over.clone(),
            where_clause: find_path_stmt.where_clause.clone(),
            shortest: find_path_stmt.shortest,
            max_steps: find_path_stmt.max_steps,
            limit: find_path_stmt.limit,
            offset: find_path_stmt.offset,
            yield_clause: find_path_stmt.yield_clause.clone(),
            weight_expression: find_path_stmt.weight_expression.clone(),
            heuristic_expression: find_path_stmt.heuristic_expression.clone(),
            with_loop: find_path_stmt.with_loop,
            with_cycle: find_path_stmt.with_cycle,
        };

        // 7. Set the output columns
        self.outputs.clear();
        if let Some(ref yc) = validated.yield_clause {
            for item in &yc.items {
                let col_name = item
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", item.expression));
                self.outputs.push(ColumnDef {
                    name: col_name,
                    type_: ValueType::Path,
                });
            }
        }

        // 8. Constructing the ValidationInfo
        let mut info = ValidationInfo::new();

        if let Some(ref over_clause) = validated.over {
            for edge_type in &over_clause.edge_types {
                info.add_alias(edge_type.clone(), AliasType::Edge);
                if !info.semantic_info.referenced_edges.contains(edge_type) {
                    info.semantic_info.referenced_edges.push(edge_type.clone());
                }
            }
        }

        self.validated_result = Some(validated);

        // 9. Return the verification results.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::FindPath
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // “FIND PATH” is not a global command; the relevant space (i.e., the context in which the command should be executed) must be selected in advance.
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

    #[test]
    fn test_find_path_validator_new() {
        let validator = FindPathValidator::new();
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());
    }

    #[test]
    fn test_statement_type() {
        let validator = FindPathValidator::new();
        assert_eq!(validator.statement_type(), StatementType::FindPath);
    }

    #[test]
    fn test_is_global_statement() {
        let validator = FindPathValidator::new();
        assert!(!validator.is_global_statement());
    }
}
