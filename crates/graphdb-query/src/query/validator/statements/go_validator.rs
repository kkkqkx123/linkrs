//! GO Statement Validator
//! Verify the GO FROM ... OVER ... WHERE ... YIELD ... statement

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::EdgeDirection;
use crate::core::DataType;
use crate::query::parser::ast::stmt::Ast;
use crate::query::parser::ast::Stmt;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::validation_info::{OptimizationHint, ValidationInfo};
use crate::query::validator::structs::AliasType;
use crate::query::validator::validator_trait::{
    ColumnDef, ExpressionProps, StatementType, StatementValidator, ValidationResult, ValueType,
};
use crate::query::QueryContext;

/// Verified information about the GO statement
#[derive(Debug, Clone)]
pub struct ValidatedGo {
    pub space_id: u64,
    pub from_source: Option<GoSource>,
    pub over_edges: Vec<OverEdge>,
    pub where_filter: Option<ContextualExpression>,
    pub yield_columns: Vec<GoYieldColumn>,
    pub step_range: Option<StepRange>,
    pub is_truncate: bool,
    pub truncate_columns: Vec<ContextualExpression>,
}

#[derive(Debug, Clone)]
pub struct GoSource {
    pub source_type: GoSourceType,
    pub expression: ContextualExpression,
    pub is_variable: bool,
    pub variable_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GoSourceType {
    VertexId,
    Expression,
    Variable,
    Parameter,
}

#[derive(Debug, Clone)]
pub struct OverEdge {
    pub edge_name: String,
    pub edge_type: Option<i32>,
    pub direction: EdgeDirection,
    pub props: Vec<EdgeProperty>,
    pub is_reversible: bool,
    pub is_all: bool,
}

#[derive(Debug, Clone)]
pub struct EdgeProperty {
    pub name: String,
    pub prop_name: String,
    pub prop_type: DataType,
}

#[derive(Debug, Clone)]
pub struct GoYieldColumn {
    pub expression: ContextualExpression,
    pub alias: String,
    pub is_distinct: bool,
}

#[derive(Debug, Clone)]
pub struct StepRange {
    pub step_from: i32,
    pub step_to: i32,
}

#[derive(Debug, Clone)]
pub struct GoInput {
    pub name: String,
    pub columns: Vec<InputColumn>,
}

#[derive(Debug, Clone)]
pub struct InputColumn {
    pub name: String,
    pub type_: DataType,
}

#[derive(Debug, Clone)]
pub struct GoOutput {
    pub name: String,
    pub type_: DataType,
    pub alias: String,
}

#[derive(Debug)]
pub struct GoValidator {
    inputs: Vec<ColumnDef>,
    outputs: Vec<ColumnDef>,
    expression_props: ExpressionProps,
    user_defined_vars: Vec<String>,
    validated_result: Option<ValidatedGo>,
    schema_manager: Option<Arc<SchemaManager>>,
}

impl GoValidator {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            expression_props: ExpressionProps::default(),
            user_defined_vars: Vec::new(),
            validated_result: None,
            schema_manager: None,
        }
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    /// Verify the FROM clause
    fn validate_from_clause(
        &mut self,
        from_vertices: &[ContextualExpression],
    ) -> Result<GoSource, ValidationError> {
        // Take the expression of the first vertex as the source.
        let from_expr = from_vertices.first().ok_or_else(|| {
            ValidationError::new(
                "The FROM clause cannot be empty".to_string(),
                ValidationErrorType::SemanticError,
            )
        })?;

        if from_expr.expression().is_none() {
            return Err(ValidationError::new(
                "FROM clause expression is invalid".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        let is_variable =
            from_expr.is_variable() && from_expr.as_variable().as_deref() != Some("$-");

        let source_type = if let Some(var_name) = from_expr.as_variable() {
            if var_name == "$-" {
                GoSourceType::Expression
            } else {
                self.user_defined_vars.push(var_name.clone());
                GoSourceType::Variable
            }
        } else if from_expr.is_literal() {
            GoSourceType::VertexId
        } else {
            GoSourceType::Expression
        };

        Ok(GoSource {
            source_type,
            expression: from_expr.clone(),
            is_variable,
            variable_name: if is_variable {
                from_expr.as_variable()
            } else {
                None
            },
        })
    }

    /// Verify the OVER clause
    fn validate_over_clause(
        &mut self,
        edge_names: &[String],
        space_name: Option<&str>,
    ) -> Result<Vec<OverEdge>, ValidationError> {
        if edge_names.is_empty() {
            return Err(ValidationError::new(
                "The OVER clause must specify at least one edge".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        let mut over_edges = Vec::new();
        for edge_name in edge_names {
            if edge_name.is_empty() {
                return Err(ValidationError::new(
                    "Side names cannot be empty".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }

            // Validate edge type exists (unless it's *)
            if edge_name != "*" {
                self.validate_edge_type_exists(edge_name, space_name)?;
            }

            over_edges.push(OverEdge {
                edge_name: edge_name.clone(),
                edge_type: None,
                direction: EdgeDirection::Out,
                props: Vec::new(),
                is_reversible: false,
                is_all: edge_name == "*",
            });
        }

        Ok(over_edges)
    }

    /// Validate that the edge type exists in the schema
    fn validate_edge_type_exists(
        &self,
        edge_name: &str,
        space_name: Option<&str>,
    ) -> Result<(), ValidationError> {
        if let (Some(ref schema_manager), Some(space)) = (&self.schema_manager, space_name) {
            match schema_manager.get_edge_type(space, edge_name) {
                Ok(Some(_)) => Ok(()),
                Ok(None) => Err(ValidationError::new(
                    format!("Edge type '{}' not found in space '{}'", edge_name, space),
                    ValidationErrorType::SemanticError,
                )),
                Err(e) => Err(ValidationError::new(
                    format!("Failed to get edge type '{}': {}", edge_name, e),
                    ValidationErrorType::SemanticError,
                )),
            }
        } else {
            // Without schema_manager, we can't validate, so just pass
            Ok(())
        }
    }

    /// Verify the WHERE clause
    fn validate_where_clause(
        &mut self,
        filter: &Option<ContextualExpression>,
    ) -> Result<Option<ContextualExpression>, ValidationError> {
        if let Some(ref expr) = filter {
            self.validate_expression(expr)?;

            // The WHERE clause should return a boolean value.
            // Assume that the expression is valid.
            Ok(Some(expr.clone()))
        } else {
            Ok(None)
        }
    }

    /// Verify the YIELD clause
    fn validate_yield_clause(
        &mut self,
        items: &[(ContextualExpression, Option<String>)],
    ) -> Result<Vec<GoYieldColumn>, ValidationError> {
        let mut column_names = HashMap::new();
        let mut yield_columns = Vec::new();

        for (i, (expr, alias)) in items.iter().enumerate() {
            self.validate_expression(expr)?;

            let col_alias = alias.clone().unwrap_or_else(|| format!("column_{}", i));

            if column_names.contains_key(&col_alias) {
                return Err(ValidationError::new(
                    format!("YIELD Column alias '{}' repeats", col_alias),
                    ValidationErrorType::DuplicateKey,
                ));
            }
            column_names.insert(col_alias.clone(), true);

            yield_columns.push(GoYieldColumn {
                expression: expr.clone(),
                alias: col_alias,
                is_distinct: false,
            });
        }

        Ok(yield_columns)
    }

    /// Verify the range of steps
    fn validate_step_range(
        &mut self,
        steps: &crate::query::parser::ast::stmt::Steps,
    ) -> Result<Option<StepRange>, ValidationError> {
        match steps {
            crate::query::parser::ast::stmt::Steps::Fixed(n) => {
                let n_i32 = *n as i32;
                if n_i32 < 0 {
                    return Err(ValidationError::new(
                        "The number of steps cannot be negative".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                Ok(Some(StepRange {
                    step_from: n_i32,
                    step_to: n_i32,
                }))
            }
            crate::query::parser::ast::stmt::Steps::Range { min, max } => {
                let min_i32 = *min as i32;
                let max_i32 = *max as i32;
                if min_i32 < 0 {
                    return Err(ValidationError::new(
                        "The start of the step range cannot be negative".to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                if max_i32 < min_i32 {
                    return Err(ValidationError::new(
                        "The end value of the step range cannot be less than the start value."
                            .to_string(),
                        ValidationErrorType::SemanticError,
                    ));
                }
                Ok(Some(StepRange {
                    step_from: min_i32,
                    step_to: max_i32,
                }))
            }
            crate::query::parser::ast::stmt::Steps::Variable(_) => {
                // Number of variable steps: determined at runtime
                Ok(None)
            }
        }
    }

    /// Verify the expression
    fn validate_expression(
        &mut self,
        expression: &ContextualExpression,
    ) -> Result<(), ValidationError> {
        if expression.expression().is_none() {
            return Err(ValidationError::new(
                "Invalid expression".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        // Check whether the variable has been defined.
        if expression.is_variable() {
            if let Some(var_name) = expression.as_variable() {
                if var_name != "$-" && !self.user_defined_vars.contains(&var_name) {
                    self.user_defined_vars.push(var_name.clone());
                }
            }
        }

        // Retrieve all variables and verify them.
        let variables = expression.get_variables();
        for var_name in variables {
            if var_name != "$-" && !self.user_defined_vars.contains(&var_name) {
                self.user_defined_vars.push(var_name);
            }
        }

        Ok(())
    }

    /// Construct the output column
    fn build_outputs(&mut self, yield_columns: &[GoYieldColumn]) {
        self.outputs.clear();
        for col in yield_columns {
            self.outputs.push(ColumnDef {
                name: col.alias.clone(),
                type_: ValueType::String,
            });
        }
    }
}

impl Default for GoValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementing the StatementValidator trait
///
/// # Refactoring Changes
/// The `validate` method accepts `Arc<Ast>` and `Arc<QueryContext>` as arguments.
impl StatementValidator for GoValidator {
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

        // 2. Obtain the GO statements
        let go_stmt = match &ast.stmt {
            Stmt::Go(go_stmt) => go_stmt,
            _ => {
                return Err(ValidationError::new(
                    "Expected GO statement".to_string(),
                    ValidationErrorType::SemanticError,
                ));
            }
        };

        // 3. Verify the FROM clause
        let from_source = self.validate_from_clause(&go_stmt.from.vertices)?;

        // 4. Verify the OVER clause
        let space_name = qctx.space_name();
        let edge_names: Vec<String> = go_stmt
            .over
            .as_ref()
            .map(|over| over.edge_types.clone())
            .unwrap_or_default();
        let over_edges = self.validate_over_clause(&edge_names, space_name.as_deref())?;

        // 5. Verify the WHERE clause
        let where_filter = self.validate_where_clause(&go_stmt.where_clause)?;

        // 6. Verify the YIELD clause
        let yield_items: Vec<(ContextualExpression, Option<String>)> = go_stmt
            .yield_clause
            .as_ref()
            .map(|yield_clause| {
                yield_clause
                    .items
                    .iter()
                    .map(|item| (item.expression.clone(), item.alias.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let yield_columns = self.validate_yield_clause(&yield_items)?;

        // 7. Verify the range of steps
        let step_range = self.validate_step_range(&go_stmt.steps)?;

        // 8. Constructing the output column
        self.build_outputs(&yield_columns);

        // 9. Obtain the space_id
        let space_id = qctx.space_id().unwrap_or(0);

        // 10. Constructing detailed ValidationInfo
        let mut info = ValidationInfo::new();

        // 10.1 Adding an Alias Map
        for edge in &over_edges {
            info.add_alias(edge.edge_name.clone(), AliasType::Edge);
        }

        // 10.2 Adding semantic information
        for edge in &over_edges {
            info.semantic_info
                .referenced_edges
                .push(edge.edge_name.clone());
        }

        // 10.3 Adding Path Analysis
        let mut path_analysis =
            crate::query::validator::structs::validation_info::PathAnalysis::new();
        path_analysis.edge_count = over_edges.len();
        path_analysis.has_direction = over_edges
            .iter()
            .any(|e| e.direction != EdgeDirection::Both);

        if let Some(ref step_range) = step_range {
            path_analysis.min_hops = Some(step_range.step_from as usize);
            path_analysis.max_hops = Some(step_range.step_to as usize);
        }

        info.add_path_analysis(path_analysis);

        // 10.4 Adding Optimization Tips
        if over_edges.len() > 10 {
            info.add_optimization_hint(OptimizationHint::PerformanceWarning {
                message: format!(
                    "GO statements contain {} edges, which may affect performance.",
                    over_edges.len()
                ),
                severity: crate::query::validator::structs::validation_info::HintSeverity::Warning,
            });
        }

        if let Some(ref step_range) = step_range {
            let steps = step_range.step_to - step_range.step_from;
            if steps > 10 {
                info.add_optimization_hint(OptimizationHint::LimitResults {
                    reason: format!("The number of hops is too large ({}), it is recommended to limit the number of results", steps),
                    suggested_limit: 1000,
                });
            }
        }

        // 10.5 Adding Validated Clauses
        info.validated_clauses
            .push(crate::query::validator::structs::ClauseKind::Match);

        // 11. Generate the verification results (perform this in the final step to avoid unnecessary clones).
        let validated = ValidatedGo {
            space_id,
            from_source: Some(from_source),
            over_edges,
            where_filter,
            yield_columns,
            step_range,
            is_truncate: false,
            truncate_columns: Vec::new(),
        };

        self.validated_result = Some(validated);

        // 12. Return the verification results containing detailed information.
        Ok(ValidationResult::success_with_info(info))
    }

    fn statement_type(&self) -> StatementType {
        StatementType::Go
    }

    fn inputs(&self) -> &[ColumnDef] {
        &self.inputs
    }

    fn outputs(&self) -> &[ColumnDef] {
        &self.outputs
    }

    fn is_global_statement(&self) -> bool {
        // “GO” is not a global statement; the relevant space (i.e., the context in which the statement is to be executed) must be selected in advance.
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
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::query::parser::ast::stmt::{Ast, FromClause, GoStmt, OverClause, Steps};
    use crate::query::parser::ast::Span;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use crate::query::QueryRequestContext;
    use std::sync::Arc;

    fn create_contextual_expr(expr: Expression) -> ContextualExpression {
        let ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());
        let meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

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

    fn create_go_stmt(from_expr: ContextualExpression, edge_types: Vec<String>) -> GoStmt {
        GoStmt {
            span: Span::default(),
            steps: Steps::Fixed(1),
            from: FromClause {
                span: Span::default(),
                vertices: vec![from_expr],
            },
            over: Some(OverClause {
                span: Span::default(),
                edge_types,
                direction: crate::core::types::EdgeDirection::Out,
            }),
            where_clause: None,
            yield_clause: None,
        }
    }

    #[test]
    fn test_go_validator_basic() {
        let mut validator = GoValidator::new();

        let go_stmt = create_go_stmt(
            create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
            vec!["friend".to_string()],
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Go(go_stmt)), qctx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_go_validator_empty_edges() {
        let mut validator = GoValidator::new();

        let go_stmt = create_go_stmt(
            create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
            vec![],
        );

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Go(go_stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err
            .message
            .contains("The OVER clause must specify at least one edge"));
    }

    #[test]
    fn test_go_validator_with_yield() {
        let mut validator = GoValidator::new();

        let mut go_stmt = create_go_stmt(
            create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
            vec!["friend".to_string()],
        );

        go_stmt.yield_clause = Some(crate::query::parser::ast::stmt::YieldClause {
            span: Span::default(),
            items: vec![crate::query::parser::ast::stmt::YieldItem {
                expression: create_contextual_expr(Expression::Variable("$$".to_string())),
                alias: Some("dst".to_string()),
            }],
            where_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
        });

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Go(go_stmt)), qctx);
        assert!(result.is_ok());

        let outputs = validator.outputs();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "dst");
    }

    #[test]
    fn test_go_validator_duplicate_alias() {
        let mut validator = GoValidator::new();

        let mut go_stmt = create_go_stmt(
            create_contextual_expr(Expression::Literal(Value::String("vid1".to_string()))),
            vec!["friend".to_string()],
        );

        go_stmt.yield_clause = Some(crate::query::parser::ast::stmt::YieldClause {
            span: Span::default(),
            items: vec![
                crate::query::parser::ast::stmt::YieldItem {
                    expression: create_contextual_expr(Expression::Variable("$$".to_string())),
                    alias: Some("same".to_string()),
                },
                crate::query::parser::ast::stmt::YieldItem {
                    expression: create_contextual_expr(Expression::Variable("$^".to_string())),
                    alias: Some("same".to_string()),
                },
            ],
            where_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
        });

        let qctx = create_test_query_context();
        let result = validator.validate(create_test_ast(Stmt::Go(go_stmt)), qctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("repeats"));
    }

    #[test]
    fn test_go_validator_trait_interface() {
        let validator = GoValidator::new();

        assert_eq!(validator.statement_type(), StatementType::Go);
        assert!(validator.inputs().is_empty());
        assert!(validator.user_defined_vars().is_empty());
    }
}
