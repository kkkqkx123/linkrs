//! Sentence Verification Strategy (Combined Version)
//! Responsible for verifying different query clauses (MATCH, RETURN, WITH, UNWIND, etc.)
//! Merge the functions of the original expression_validator and clause_validator.

use crate::core::types::expr::{ContextualExpression, ExpressionMeta};
use crate::core::Expression;
use crate::core::YieldColumn;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::validator::error::{ValidationError, ValidationErrorType};
use crate::query::validator::structs::alias_structs::{AliasType, BoundaryClauseContext};
use crate::query::validator::structs::{
    MatchClauseContext, ReturnClauseContext, YieldClauseContext,
};
use crate::query::validator::{Path, QueryPart};
use std::sync::Arc;

/// Sentence validation strategy
pub struct ClauseValidationStrategy;

impl Default for ClauseValidationStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl ClauseValidationStrategy {
    pub fn new() -> Self {
        Self
    }

    /// Create a ContextualExpression from an Expression.
    fn create_contextual_expression(&self, expr: Expression) -> ContextualExpression {
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = expr_ctx.register_expression(meta);
        ContextualExpression::new(id, expr_ctx)
    }

    /// Verify the returned subquery.
    pub fn validate_return_clause(
        &self,
        context: &ReturnClauseContext,
    ) -> Result<(), ValidationError> {
        // Check the availability of the alias.
        use super::alias_strategy::AliasValidationStrategy;
        let alias_validator = AliasValidationStrategy::new();

        for col in &context.yield_clause.yield_columns {
            alias_validator.validate_aliases(
                std::slice::from_ref(&col.expression),
                &context.aliases_available,
            )?;
        }

        // Verify pagination
        if let Some(ref pagination) = context.pagination {
            if pagination.skip < 0 {
                return Err(ValidationError::new(
                    "SKIP cannot be negative".to_string(),
                    ValidationErrorType::PaginationError,
                ));
            }
            if pagination.limit < 0 {
                return Err(ValidationError::new(
                    "LIMIT cannot be negative".to_string(),
                    ValidationErrorType::PaginationError,
                ));
            }
        }

        // Verify the sorting order.
        if let Some(ref order_by) = context.order_by {
            // Here, the sorting criteria can be verified.
            for &(index, _) in &order_by.indexed_order_factors {
                // Check whether the index is valid.
                if index >= context.yield_clause.yield_columns.len() {
                    return Err(ValidationError::new(
                        format!("Column index {} out of range", index),
                        ValidationErrorType::PaginationError,
                    ));
                }
            }
        }

        Ok(())
    }

    /// Create all columns with their respective aliases.
    pub fn build_columns_for_all_named_aliases(
        &self,
        query_parts: &[QueryPart],
        columns: &mut Vec<YieldColumn>,
    ) -> Result<(), ValidationError> {
        if query_parts.is_empty() {
            return Err(ValidationError::new(
                "No aliases are declared.".to_string(),
                ValidationErrorType::SemanticError,
            ));
        }

        let curr_query_part = query_parts.last().ok_or_else(|| {
            ValidationError::new(
                "Query parts should not be empty".to_string(),
                ValidationErrorType::SemanticError,
            )
        })?;

        // Process the boundary clause from the previous query part.
        if query_parts.len() > 1 {
            let prev_query_part = &query_parts[query_parts.len() - 2];
            if let Some(ref boundary) = prev_query_part.boundary {
                match boundary {
                    BoundaryClauseContext::Unwind(unwind_data) => {
                        // Add an alias for the Unwind clause
                        columns.push(YieldColumn::new(
                            self.create_contextual_expression(Expression::Label(
                                unwind_data.alias.clone(),
                            )),
                            unwind_data.alias.clone(),
                        ));

                        // Add the previously available aliases.
                        for alias in prev_query_part.aliases_available.keys() {
                            columns.push(YieldColumn::new(
                                self.create_contextual_expression(Expression::Label(alias.clone())),
                                alias.clone(),
                            ));
                        }

                        // Add the aliases that were generated previously.
                        for alias in prev_query_part.aliases_generated.keys() {
                            columns.push(YieldColumn::new(
                                self.create_contextual_expression(Expression::Label(alias.clone())),
                                alias.clone(),
                            ));
                        }
                    }
                    BoundaryClauseContext::With(with_data) => {
                        // Column with the "With" clause added
                        for col in &with_data.yield_clause.yield_columns {
                            if !col.alias.is_empty() {
                                columns.push(YieldColumn::new(
                                    self.create_contextual_expression(Expression::Label(
                                        col.alias.clone(),
                                    )),
                                    col.alias.clone(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Process the matching clauses in the current query section.
        for match_ctx in &curr_query_part.matchs {
            for path in &match_ctx.paths {
                // Add aliases for the nodes and edges in the path.
                for i in 0..path.edge_infos.len() {
                    if !path.node_infos[i].anonymous {
                        columns.push(YieldColumn::new(
                            self.create_contextual_expression(Expression::Label(
                                path.node_infos[i].alias.clone(),
                            )),
                            path.node_infos[i].alias.clone(),
                        ));
                    }

                    if !path.edge_infos[i].anonymous {
                        columns.push(YieldColumn::new(
                            self.create_contextual_expression(Expression::Label(
                                path.edge_infos[i].alias.clone(),
                            )),
                            path.edge_infos[i].alias.clone(),
                        ));
                    }
                }

                // Add an alias for the last node.
                let last_node = path.node_infos.last().ok_or_else(|| {
                    ValidationError::new(
                        "Path should have at least one node".to_string(),
                        ValidationErrorType::SemanticError,
                    )
                })?;
                if !last_node.anonymous {
                    columns.push(YieldColumn::new(
                        self.create_contextual_expression(Expression::Label(
                            last_node.alias.clone(),
                        )),
                        last_node.alias.clone(),
                    ));
                }
            }

            // Add a path alias
            for (alias, alias_type) in &match_ctx.aliases_generated {
                if *alias_type == AliasType::Path {
                    columns.push(YieldColumn::new(
                        self.create_contextual_expression(Expression::Label(alias.clone())),
                        alias.clone(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Build the output.
    pub fn build_outputs(&self, paths: &mut Vec<Path>) -> Result<(), ValidationError> {
        // Construct the query output, including column names and data types.
        // The final output format will be generated based on the path information provided here.
        for _path in paths {
            // Generate output information for each path.
            // In the actual implementation, a specific output format will be constructed here.
        }
        Ok(())
    }

    /// Verify the Yield clause
    pub fn validate_yield_clause(
        &self,
        context: &YieldClauseContext,
    ) -> Result<(), ValidationError> {
        // If there are aggregate functions, perform special validation.
        if context.has_agg {
            return self.validate_group(context);
        }

        // For the regular Yield clause, verify the alias.
        use super::alias_strategy::AliasValidationStrategy;
        let alias_validator = AliasValidationStrategy::new();
        for col in &context.yield_columns {
            alias_validator.validate_aliases(
                std::slice::from_ref(&col.expression),
                &context.aliases_available,
            )?;
        }

        Ok(())
    }

    /// Verify the grouping clause
    fn validate_group(&self, yield_ctx: &YieldClauseContext) -> Result<(), ValidationError> {
        // Verify the grouping logic
        use super::aggregate_strategy::AggregateValidationStrategy;
        let aggregate_validator = AggregateValidationStrategy::new();

        for col in &yield_ctx.yield_columns {
            // If the expression contains aggregate functions, verify the aggregate expression.
            if aggregate_validator.has_aggregate_expression(&col.expression) {
                // Verify the aggregate functions
                // In the actual implementation, more detailed verification of the aggregate functions will be carried out here.
            } else {
                // Non-aggregated expressions will be added as grouping keys.
                // The context needs to be modified here, but in the Strategy pattern, direct modification should not be performed.
                // This should be processed in the main validator.
            }
        }

        Ok(())
    }

    /// Verify the context of the Match clause.
    pub fn validate_match_clause_context(
        &self,
        context: &MatchClauseContext,
    ) -> Result<(), ValidationError> {
        // Verify the basic structure of the Match clause
        // Check the validity of paths, aliases, and other elements.

        // Verify the path.
        for _path in &context.paths {
            // Verify the path structure
            // In the actual implementation, more detailed path validation will be carried out here.
        }

        // Verify the WHERE clause (if it exists).
        if let Some(ref _where_clause) = context.where_clause {
            // Verify the WHERE clause
            // In the actual implementation, a more detailed verification of the WHERE clause will be carried out here.
        }

        Ok(())
    }
}

impl ClauseValidationStrategy {
    /// Obtain the policy name
    pub fn strategy_name(&self) -> &'static str {
        "ClauseValidationStrategy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;
    use crate::core::YieldColumn;
    use crate::query::validator::structs::path_structs::PathYieldType;
    use crate::query::validator::structs::PaginationContext;
    use crate::query::validator::structs::YieldClauseContext;
    use std::collections::HashMap;

    #[test]
    fn test_clause_validation_strategy_creation() {
        let strategy = ClauseValidationStrategy::new();
        assert_eq!(strategy.strategy_name(), "ClauseValidationStrategy");
    }

    #[test]
    fn test_validate_return_clause() {
        let strategy = ClauseValidationStrategy::new();

        // Create test data
        let return_context = ReturnClauseContext {
            yield_clause: YieldClauseContext {
                yield_columns: vec![YieldColumn::new(
                    strategy.create_contextual_expression(Expression::Literal(
                        crate::core::Value::Int(1),
                    )),
                    "col1".to_string(),
                )],
                aliases_available: HashMap::new(),
                aliases_generated: HashMap::new(),
                distinct: false,
                has_agg: false,
                group_keys: Vec::new(),
                group_items: Vec::new(),
                need_gen_project: false,
                agg_output_column_names: Vec::new(),
                proj_output_column_names: Vec::new(),
                paths: Vec::new(),
                query_parts: Vec::new(),
                errors: Vec::new(),
                filter_condition: None,
                skip: None,
                limit: None,
            },
            aliases_available: HashMap::new(),
            aliases_generated: HashMap::new(),
            pagination: None,
            order_by: None,
            distinct: false,
            query_parts: Vec::new(),
            errors: Vec::new(),
        };

        assert!(strategy.validate_return_clause(&return_context).is_ok());
    }

    #[test]
    fn test_validate_return_clause_with_pagination() {
        let strategy = ClauseValidationStrategy::new();

        // Create test data with pagination.
        let return_context = ReturnClauseContext {
            yield_clause: YieldClauseContext {
                yield_columns: vec![YieldColumn::new(
                    strategy.create_contextual_expression(Expression::Literal(
                        crate::core::Value::Int(1),
                    )),
                    "col1".to_string(),
                )],
                aliases_available: HashMap::new(),
                aliases_generated: HashMap::new(),
                distinct: false,
                has_agg: false,
                group_keys: Vec::new(),
                group_items: Vec::new(),
                need_gen_project: false,
                agg_output_column_names: Vec::new(),
                proj_output_column_names: Vec::new(),
                paths: Vec::new(),
                query_parts: Vec::new(),
                errors: Vec::new(),
                filter_condition: None,
                skip: None,
                limit: None,
            },
            aliases_available: HashMap::new(),
            aliases_generated: HashMap::new(),
            pagination: Some(PaginationContext { skip: 0, limit: 10 }),
            order_by: None,
            distinct: false,
            query_parts: Vec::new(),
            errors: Vec::new(),
        };

        assert!(strategy.validate_return_clause(&return_context).is_ok());
    }

    #[test]
    fn test_validate_return_clause_invalid_pagination() {
        let strategy = ClauseValidationStrategy::new();

        // Create test data for invalid pagination.
        let return_context = ReturnClauseContext {
            yield_clause: YieldClauseContext {
                yield_columns: vec![YieldColumn::new(
                    strategy.create_contextual_expression(Expression::Literal(
                        crate::core::Value::Int(1),
                    )),
                    "col1".to_string(),
                )],
                aliases_available: HashMap::new(),
                aliases_generated: HashMap::new(),
                distinct: false,
                has_agg: false,
                group_keys: Vec::new(),
                group_items: Vec::new(),
                need_gen_project: false,
                agg_output_column_names: Vec::new(),
                proj_output_column_names: Vec::new(),
                paths: Vec::new(),
                query_parts: Vec::new(),
                errors: Vec::new(),
                filter_condition: None,
                skip: None,
                limit: None,
            },
            aliases_available: HashMap::new(),
            aliases_generated: HashMap::new(),
            pagination: Some(PaginationContext {
                skip: -1,
                limit: 10,
            }),
            order_by: None,
            distinct: false,
            query_parts: Vec::new(),
            errors: Vec::new(),
        };

        assert!(strategy.validate_return_clause(&return_context).is_err());
    }

    #[test]
    fn test_build_columns_for_all_named_aliases() {
        let strategy = ClauseValidationStrategy::new();

        // Create the test query section
        let query_parts = vec![QueryPart {
            matchs: Vec::new(),
            boundary: None,
            aliases_available: HashMap::new(),
            aliases_generated: HashMap::new(),
            paths: Vec::new(),
        }];

        let mut columns = Vec::new();

        // Testing the empty query section…
        assert!(strategy
            .build_columns_for_all_named_aliases(&[], &mut columns)
            .is_err());

        // The test includes a query section, but no aliases.
        assert!(strategy
            .build_columns_for_all_named_aliases(&query_parts, &mut columns)
            .is_ok());
    }

    #[test]
    fn test_validate_yield_clause() {
        let strategy = ClauseValidationStrategy::new();

        let yield_context = YieldClauseContext {
            yield_columns: vec![YieldColumn::new(
                strategy
                    .create_contextual_expression(Expression::Literal(crate::core::Value::Int(1))),
                "col1".to_string(),
            )],
            aliases_available: HashMap::new(),
            aliases_generated: HashMap::new(),
            distinct: false,
            has_agg: false,
            group_keys: Vec::new(),
            group_items: Vec::new(),
            need_gen_project: false,
            agg_output_column_names: Vec::new(),
            proj_output_column_names: Vec::new(),
            paths: Vec::new(),
            query_parts: Vec::new(),
            errors: Vec::new(),
            filter_condition: None,
            skip: None,
            limit: None,
        };

        assert!(strategy.validate_yield_clause(&yield_context).is_ok());
    }

    #[test]
    fn test_validate_match_clause_context() {
        let strategy = ClauseValidationStrategy::new();

        let match_context = MatchClauseContext {
            paths: Vec::new(),
            aliases_available: HashMap::new(),
            aliases_generated: HashMap::new(),
            where_clause: None,
            is_optional: false,
            skip: None,
            limit: None,
            query_parts: Vec::new(),
            errors: Vec::new(),
        };

        assert!(strategy
            .validate_match_clause_context(&match_context)
            .is_ok());
    }

    #[test]
    fn test_build_outputs() {
        let strategy = ClauseValidationStrategy::new();

        let mut paths = Vec::new();

        // Testing the empty path
        assert!(strategy.build_outputs(&mut paths).is_ok());

        // Testing the case where there is a path
        let path = Path {
            alias: "test_path".to_string(),
            anonymous: false,
            gen_path: true,
            path_type: PathYieldType::Default,
            node_infos: Vec::new(),
            edge_infos: Vec::new(),
            path_build: None,
            is_pred: false,
            is_anti_pred: false,
            compare_variables: Vec::new(),
            collect_variable: String::new(),
            roll_up_apply: false,
        };

        paths.push(path);
        assert!(strategy.build_outputs(&mut paths).is_ok());
    }
}
