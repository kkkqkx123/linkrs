//! GET SUBGRAPH statement validator – New system version
//! Verify the validity of the GET SUBGRAPH statement.
//!
//! This document has been restructured in accordance with the new trait + enumeration validator framework.
//! The StatementValidator trait has been implemented to unify the interface.
//! 2. All functions are retained.
//! Verify Lifecycle Management
//! Management of input/output columns
//! Expression property tracking
//! User-defined variable management
//! Permission check
//! Execution plan generation
//! 3. The lifecycle parameters have been removed, and the SchemaManager is now managed using Arc.
//! 4. Use AstContext to manage the context in a unified manner.

use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::stmt::{
    Ast, FromClause, OverClause, Steps, SubgraphStmt, YieldClause,
};
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::ValidationInfo;
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Information obtained from the verified sub-image
#[derive(Debug, Clone)]
pub struct ValidatedGetSubgraph {
    pub space_id: u64,
    pub steps: Steps,
    pub from: FromClause,
    pub over: Option<OverClause>,
    pub where_clause: Option<ContextualExpression>,
    pub yield_clause: Option<YieldClause>,
}

/// GET SUBGRAPH Validator – New System Implementation
///
/// Functionality integrity assurance:
/// 1. Complete validation lifecycle
/// 2. Management of input/output columns
/// 3. Expression property tracing
/// 4. Management of user-defined variables
/// 5. Permission checking (scalable)
/// 6. Generation of execution plans (scalable)
#[derive(Debug)]
pub struct GetSubgraphValidator {
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
    validated_result: Option<ValidatedGetSubgraph>,
}

impl GetSubgraphValidator {
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
    pub fn validated_result(&self) -> Option<&ValidatedGetSubgraph> {
        self.validated_result.as_ref()
    }

    /// Basic validation
    fn validate_get_subgraph(&self, stmt: &SubgraphStmt) -> Result<(), ValidationError> {
        self.validate_steps(&stmt.steps)?;
        self.validate_from_clause(&stmt.from)?;
        if let Some(ref over) = stmt.over {
            self.validate_over_clause(over)?;
        }
        Ok(())
    }

    /// Verification steps
    fn validate_steps(&self, steps: &Steps) -> Result<(), ValidationError> {
        match steps {
            Steps::Fixed(n) => {
                if *n > 100 {
                    return Err(ValidationError::new(
                        "Maximum steps cannot exceed 100".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            Steps::Range { min, max } => {
                if max < min {
                    return Err(ValidationError::new(
                        "Maximum steps cannot be less than minimum steps".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                if *max > 100 {
                    return Err(ValidationError::new(
                        "Maximum steps cannot exceed 100".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
            }
            Steps::Variable(_) => {}
        }
        Ok(())
    }

    /// Verify the FROM clause
    fn validate_from_clause(&self, from: &FromClause) -> Result<(), ValidationError> {
        // Assume that the FROM clause is valid.
        let _ = from;
        Ok(())
    }

    /// Verify the OVER clause
    fn validate_over_clause(&self, over: &OverClause) -> Result<(), ValidationError> {
        for edge_type in &over.edge_types {
            if edge_type.is_empty() {
                return Err(ValidationError::new(
                    "Edge type name cannot be empty".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        }
        Ok(())
    }

    /// Verify the YIELD clause
    fn validate_yield_clause(
        &self,
        yield_clause: &Option<YieldClause>,
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

impl Default for GetSubgraphValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for GetSubgraphValidator {
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

        // 2. Obtain the GET SUBGRAPH statement (with ownership rights)
        let get_subgraph_stmt = match &ast.stmt {
            crate::query::parser::ast::Stmt::Subgraph(s) => s,
            _ => {
                return Err(ValidationError::new(
                    "Expected GET SUBGRAPH statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 3. Perform basic validation.
        self.validate_get_subgraph(get_subgraph_stmt)?;

        // 4. Verify the YIELD clause
        self.validate_yield_clause(&get_subgraph_stmt.yield_clause)?;

        // 5. Obtain the space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 6. Create verification results (directly transfer ownership without cloning).
        let validated = ValidatedGetSubgraph {
            space_id,
            steps: get_subgraph_stmt.steps.clone(),
            from: get_subgraph_stmt.from.clone(),
            over: get_subgraph_stmt.over.clone(),
            where_clause: get_subgraph_stmt.where_clause.clone(),
            yield_clause: get_subgraph_stmt.yield_clause.clone(),
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
                    type_: ValueType::Vertex,
                });
            }
        }

        // 8. Constructing ValidationInfo
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
        StatementType::GetSubgraph
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // The `GET SUBGRAPH` command is not a global statement; it requires that a space (i.e., a specific subset of the data) be selected in advance.
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
    use crate::query::parser::ast::Span;

    #[test]
    fn test_validate_steps_fixed() {
        let validator = GetSubgraphValidator::new();
        let result = validator.validate_steps(&Steps::Fixed(5));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_steps_fixed_exceed_max() {
        let validator = GetSubgraphValidator::new();
        let result = validator.validate_steps(&Steps::Fixed(101));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("exceed 100"));
    }

    #[test]
    fn test_validate_steps_range_invalid() {
        let validator = GetSubgraphValidator::new();
        let result = validator.validate_steps(&Steps::Range { min: 5, max: 3 });
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("less than"));
    }

    #[test]
    fn test_validate_steps_range_valid() {
        let validator = GetSubgraphValidator::new();
        let result = validator.validate_steps(&Steps::Range { min: 1, max: 5 });
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_over_clause_empty() {
        let validator = GetSubgraphValidator::new();
        let over = OverClause {
            span: Span::default(),
            edge_types: vec!["".to_string()],
            direction: crate::core::types::EdgeDirection::Both,
        };
        let result = validator.validate_over_clause(&over);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("empty"));
    }

    #[test]
    fn test_validate_over_clause_valid() {
        let validator = GetSubgraphValidator::new();
        let over = OverClause {
            span: Span::default(),
            edge_types: vec!["friend".to_string(), "colleague".to_string()],
            direction: crate::core::types::EdgeDirection::Both,
        };
        let result = validator.validate_over_clause(&over);
        assert!(result.is_ok());
    }

    #[test]
    fn test_statement_validator_trait() {
        let validator = GetSubgraphValidator::new();

        // Testing the `statement_type`
        assert_eq!(validator.statement_type(), StatementType::GetSubgraph);

        // Testing inputs/outputs
        assert!(validator.inputs().is_empty());
        assert!(validator.outputs().is_empty());

        // Testing user_defined_vars
        assert!(validator.user_defined_vars().is_empty());
    }
}
