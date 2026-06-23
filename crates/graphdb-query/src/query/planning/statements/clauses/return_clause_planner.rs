//! RETURN Statement Planner
//!
//! Responsible for planning the execution of the RETURN statement and implementing the projection of the results.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::expression::ExpressionMeta;
use crate::core::types::expr::expression_utils::generate_default_alias_from_contextual;
use crate::core::types::operators::AggregateFunction;
use crate::core::Expression;
use crate::core::YieldColumn;
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode;
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::query::planning::plan::core::nodes::AggregateNode;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;
use crate::query::planning::statements::statement_planner::ClausePlanner;
use crate::query::validator::structs::CypherClauseKind;
use crate::query::QueryContext;
use std::sync::Arc;

pub use crate::query::planning::plan::core::PlanNodeEnum;

use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;

/// RETURN Statement Planner
///
/// Responsible for planning the execution of the RETURN statement and implementing the projection of the results.
#[derive(Debug, Clone)]
pub struct ReturnClausePlanner {
    distinct: bool,
}

impl Default for ReturnClausePlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl ReturnClausePlanner {
    pub fn new() -> Self {
        Self { distinct: false }
    }

    pub fn with_distinct(distinct: bool) -> Self {
        Self { distinct }
    }

    pub fn from_stmt(stmt: &Stmt) -> Self {
        let distinct = extract_distinct_flag(stmt);
        Self::with_distinct(distinct)
    }

    pub fn set_distinct(&mut self, distinct: bool) {
        self.distinct = distinct;
    }
}

fn extract_distinct_flag(stmt: &Stmt) -> bool {
    if let Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            return return_clause.distinct;
        }
    }
    false
}

fn extract_having_clause(stmt: &Stmt) -> Option<crate::core::types::ContextualExpression> {
    if let Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            return return_clause.having_clause.clone();
        }
    }
    None
}

fn extract_return_columns(stmt: &Stmt) -> Result<Vec<YieldColumn>, PlannerError> {
    let mut columns = Vec::new();

    if let Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            for item in &return_clause.items {
                match item {
                    crate::query::parser::ast::stmt::ReturnItem::Expression {
                        expression,
                        alias,
                    } => {
                        let alias = alias
                            .clone()
                            .or_else(|| Some(generate_default_alias_from_contextual(expression)));
                        columns.push(YieldColumn {
                            expression: expression.clone(),
                            alias: alias.unwrap_or_else(|| "expr".to_string()),
                            is_matched: false,
                        });
                    }
                }
            }
        }
    }

    if columns.is_empty() {
        return Err(PlannerError::PlanGenerationFailed(
            "RETURN clause missing return item".to_string(),
        ));
    }

    Ok(columns)
}

impl ClausePlanner for ReturnClausePlanner {
    fn clause_kind(&self) -> CypherClauseKind {
        CypherClauseKind::Return
    }

    fn transform_clause(
        &self,
        _qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let yield_columns = extract_return_columns(stmt)?;

        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "The RETURN clause requires an input plan".to_string(),
            )
        })?;

        let has_aggregate = yield_columns.iter().any(|col| {
            if let Some(expr_meta) = col.expression.expression() {
                expression_contains_aggregate(expr_meta.inner())
            } else {
                false
            }
        });

        if has_aggregate {
            let (group_keys, agg_functions, agg_aliases) = extract_aggregate_info(&yield_columns)?;

            let mut project_columns: Vec<YieldColumn> = yield_columns
                .iter()
                .filter(|col| {
                    if let Some(expr_meta) = col.expression.expression() {
                        !expression_contains_aggregate(expr_meta.inner())
                    } else {
                        false
                    }
                })
                .cloned()
                .collect();

            // Also project the argument expressions from aggregate functions
            // so they are available as input columns for the AggregateExecutor
            let existing_aliases: Vec<String> =
                project_columns.iter().map(|pc| pc.alias.clone()).collect();
            for col in &yield_columns {
                if let Some(expr_meta) = col.expression.expression() {
                    let inner = expr_meta.inner();
                    if let Expression::Aggregate { arg, .. } = inner {
                        let arg_expr_str = arg.to_expression_string();
                        // Only add if not already projected (avoid duplicates)
                        if !existing_aliases.contains(&arg_expr_str) {
                            let ctx = col.expression.context();
                            let meta = ExpressionMeta::new(arg.as_ref().clone());
                            let id = ctx.register_expression(meta);
                            let ctx_expr = ContextualExpression::new(id, ctx.clone());
                            project_columns.push(YieldColumn {
                                expression: ctx_expr,
                                alias: arg_expr_str,
                                is_matched: false,
                            });
                        }
                    }
                }
            }

            let project_node = ProjectNode::new(input_node.clone(), project_columns)?;
            let project_plan = SubPlan::new(Some(project_node.into_enum()), input_plan.tail);

            let aggregate_node = AggregateNode::with_agg_aliases(
                project_plan.root.clone().unwrap(),
                group_keys,
                agg_functions,
                agg_aliases,
            )?;

            let mut final_node: PlanNodeEnum = aggregate_node.into_enum();

            // Apply HAVING clause filter if present
            if let Some(having_expr) = extract_having_clause(stmt) {
                let filter_node =
                    FilterNode::new(final_node.clone(), having_expr).map_err(|e| {
                        PlannerError::PlanGenerationFailed(format!(
                            "Failed to create FilterNode for HAVING: {}",
                            e
                        ))
                    })?;
                final_node = PlanNodeEnum::Filter(filter_node);
            }

            if self.distinct {
                if let Ok(dedup) = DedupNode::new(final_node.clone()) {
                    final_node = dedup.into_enum();
                }
            }

            Ok(SubPlan::new(Some(final_node), project_plan.tail))
        } else {
            let project_node = ProjectNode::new(input_node.clone(), yield_columns)?;

            let final_node = if self.distinct {
                match DedupNode::new(project_node.clone().into_enum()) {
                    Ok(dedup) => dedup.into_enum(),
                    Err(_) => project_node.into_enum(),
                }
            } else {
                project_node.into_enum()
            };

            Ok(SubPlan::new(Some(final_node), input_plan.tail))
        }
    }
}

fn expression_contains_aggregate(expr: &crate::core::Expression) -> bool {
    use crate::core::Expression;
    match expr {
        Expression::Aggregate { .. } => true,
        Expression::Binary { left, right, .. } => {
            expression_contains_aggregate(left) || expression_contains_aggregate(right)
        }
        Expression::Unary { operand, .. } => expression_contains_aggregate(operand),
        Expression::Function { args, .. } => args.iter().any(expression_contains_aggregate),
        _ => false,
    }
}

/// Aggregate extraction result containing group keys, aggregate functions, and their aliases
type AggregateExtractionResult = (Vec<String>, Vec<AggregateFunction>, Vec<String>);

fn extract_aggregate_info(
    columns: &[YieldColumn],
) -> Result<AggregateExtractionResult, PlannerError> {
    let mut group_keys = Vec::new();
    let mut agg_functions = Vec::new();
    let mut agg_aliases = Vec::new();

    for col in columns {
        if let Some(expr_meta) = col.expression.expression() {
            let expr = expr_meta.inner();
            if expression_contains_aggregate(expr) {
                if let Some(agg_func) = extract_aggregate_function(expr) {
                    agg_functions.push(agg_func);
                    agg_aliases.push(col.alias.clone());
                }
            } else {
                let key = col.alias.clone();
                if !group_keys.contains(&key) {
                    group_keys.push(key);
                }
            }
        }
    }

    Ok((group_keys, agg_functions, agg_aliases))
}

fn extract_aggregate_function(expr: &crate::core::Expression) -> Option<AggregateFunction> {
    use crate::core::Expression;
    match expr {
        Expression::Aggregate { func, .. } => Some(func.clone()),
        Expression::Binary { left, right, .. } => {
            extract_aggregate_function(left).or_else(|| extract_aggregate_function(right))
        }
        Expression::Unary { operand, .. } => extract_aggregate_function(operand),
        Expression::Function { args, .. } => args.iter().find_map(extract_aggregate_function),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Expression;
    use crate::query::parser::ast::Span;
    use crate::query::planning::plan::core::nodes::StartNode;
    use crate::query::planning::plan::core::PlanNodeEnum;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_return_clause_planner_creation() {
        let planner = ReturnClausePlanner::new();
        assert_eq!(planner.clause_kind(), CypherClauseKind::Return);
    }

    #[test]
    fn test_return_clause_planner_with_distinct() {
        let planner = ReturnClausePlanner::with_distinct(true);
        assert!(planner.distinct);
    }

    #[test]
    fn test_extract_distinct_flag() {
        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: Some(crate::query::parser::ast::stmt::ReturnClause {
                span: Span::default(),
                items: vec![],
                distinct: true,
                order_by: None,
                limit: None,
                skip: None,
                sample: None,
                having_clause: None,
            }),
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let distinct = extract_distinct_flag(&match_stmt);
        assert!(distinct);
    }

    #[test]
    fn test_extract_return_columns() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: Some(crate::query::parser::ast::stmt::ReturnClause {
                span: Span::default(),
                items: vec![crate::query::parser::ast::stmt::ReturnItem::Expression {
                    expression: ctx_expr.clone(),
                    alias: None,
                }],
                distinct: false,
                order_by: None,
                limit: None,
                skip: None,
                sample: None,
                having_clause: None,
            }),
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let columns = extract_return_columns(&match_stmt).expect("failed to extract");
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].alias, "n");
    }

    #[test]
    fn test_extract_return_columns_empty() {
        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: Some(crate::query::parser::ast::stmt::ReturnClause {
                span: Span::default(),
                items: vec![],
                distinct: false,
                order_by: None,
                limit: None,
                skip: None,
                sample: None,
                having_clause: None,
            }),
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let result = extract_return_columns(&match_stmt);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_default_alias() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let contextual = ContextualExpression::new(id, ctx.clone());
        let alias = generate_default_alias_from_contextual(&contextual);
        assert_eq!(alias, "n");

        let expr = Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "name".to_string(),
        };
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let contextual = ContextualExpression::new(id, ctx.clone());
        let alias = generate_default_alias_from_contextual(&contextual);
        assert_eq!(alias, "n.name");

        let expr = Expression::Function {
            name: "count".to_string(),
            args: vec![],
        };
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let contextual = ContextualExpression::new(id, ctx.clone());
        let alias = generate_default_alias_from_contextual(&contextual);
        assert_eq!(alias, "count");
    }

    #[test]
    fn test_transform_clause() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: Some(crate::query::parser::ast::stmt::ReturnClause {
                span: Span::default(),
                items: vec![crate::query::parser::ast::stmt::ReturnItem::Expression {
                    expression: ctx_expr.clone(),
                    alias: None,
                }],
                distinct: false,
                order_by: None,
                limit: None,
                skip: None,
                sample: None,
                having_clause: None,
            }),
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
        };

        let planner = ReturnClausePlanner::new();
        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_ok());

        let sub_plan = result.expect("transform_clause should succeed");
        assert!(sub_plan.root.is_some());

        match sub_plan.root {
            Some(PlanNodeEnum::Project(_)) => {}
            Some(PlanNodeEnum::Dedup(_)) => {}
            _ => panic!("Expected ProjectNode or DedupNode"),
        }
    }

    #[test]
    fn test_transform_clause_with_distinct() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: Some(crate::query::parser::ast::stmt::ReturnClause {
                span: Span::default(),
                items: vec![crate::query::parser::ast::stmt::ReturnItem::Expression {
                    expression: ctx_expr.clone(),
                    alias: None,
                }],
                distinct: true,
                order_by: None,
                limit: None,
                skip: None,
                sample: None,
                having_clause: None,
            }),
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
        };

        let planner = ReturnClausePlanner::with_distinct(true);
        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_ok());

        let sub_plan = result.expect("transform_clause should succeed");
        assert!(sub_plan.root.is_some());

        if let Some(PlanNodeEnum::Dedup(_)) = sub_plan.root {
        } else {
            panic!("Expected DedupNode with distinct=true");
        }
    }

    #[test]
    fn test_transform_clause_empty_input_plan() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: Some(crate::query::parser::ast::stmt::ReturnClause {
                span: Span::default(),
                items: vec![crate::query::parser::ast::stmt::ReturnItem::Expression {
                    expression: ctx_expr.clone(),
                    alias: None,
                }],
                distinct: false,
                order_by: None,
                limit: None,
                skip: None,
                sample: None,
                having_clause: None,
            }),
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let input_plan = SubPlan {
            root: None,
            tail: None,
        };

        let planner = ReturnClausePlanner::new();
        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_err());
    }
}
