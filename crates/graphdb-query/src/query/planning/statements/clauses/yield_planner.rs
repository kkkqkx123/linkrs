//! The YIELD clause planner
//!
//! Responsible for converting the YIELD clause into an execution plan node.
//! Support for the YIELD ... WHERE ... syntax

use crate::core::YieldColumn;
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::{FilterNode, LimitNode, PlanNodeEnum, ProjectNode};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;
use crate::query::planning::statements::statement_planner::ClausePlanner;
use crate::query::validator::structs::CypherClauseKind;
use crate::query::QueryContext;
use std::sync::Arc;

/// Yield info result type alias
type YieldInfoResult = Result<
    (
        Vec<YieldColumn>,
        Option<crate::core::types::ContextualExpression>,
        Option<usize>,
        Option<usize>,
    ),
    PlannerError,
>;

/// The YIELD clause planner
#[derive(Debug)]
pub struct YieldClausePlanner {}

impl YieldClausePlanner {
    pub fn new() -> Self {
        Self {}
    }

    /// Planning the YIELD clause
    ///
    /// Processing procedure:
    /// 1. Constructing the projection node (YIELD column)
    /// 2. If there are WHERE conditions, add a Filter node.
    /// 3. If there are LIMIT/SKIP settings, add pagination nodes.
    pub fn plan_yield_clause(
        &self,
        yield_columns: &[YieldColumn],
        filter_condition: Option<crate::core::types::ContextualExpression>,
        skip: Option<usize>,
        limit: Option<usize>,
        input_plan: &SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let mut current_plan = input_plan.clone();

        // 1. Build the projection node (if there is a specific YIELD column).
        if !yield_columns.is_empty() {
            let project_node = self.create_project_node(&current_plan, yield_columns)?;
            current_plan = SubPlan::new(Some(project_node), current_plan.tail.clone());
        }

        // 2. If there are WHERE conditions, add a Filter node.
        if let Some(ref filter_condition) = filter_condition {
            let filter_node = self.create_filter_node(&current_plan, filter_condition.clone())?;
            current_plan = SubPlan::new(Some(filter_node), current_plan.tail.clone());
        }

        // 3. Handling pagination (LIMIT/SKIP)
        if limit.is_some() || skip.is_some() {
            current_plan = self.apply_pagination(current_plan, skip, limit)?;
        }

        Ok(current_plan)
    }

    /// Create a projection node.
    fn create_project_node(
        &self,
        input_plan: &SubPlan,
        columns: &[YieldColumn],
    ) -> Result<PlanNodeEnum, PlannerError> {
        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        ProjectNode::new(input_node.clone(), columns.to_vec())
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!(
                    "Failed to create projection node: {}",
                    e
                ))
            })
            .map(PlanNodeEnum::Project)
    }

    /// Create a filter node.
    fn create_filter_node(
        &self,
        input_plan: &SubPlan,
        condition: crate::core::types::ContextualExpression,
    ) -> Result<PlanNodeEnum, PlannerError> {
        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        FilterNode::new(input_node.clone(), condition)
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create filter node: {}", e))
            })
            .map(PlanNodeEnum::Filter)
    }

    /// Application pagination
    fn apply_pagination(
        &self,
        input_plan: SubPlan,
        skip: Option<usize>,
        limit: Option<usize>,
    ) -> Result<SubPlan, PlannerError> {
        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("The input plan has no root node".to_string())
        })?;

        let offset = skip.unwrap_or(0) as i64;
        let count = limit.map(|l| l as i64).unwrap_or(i64::MAX);

        let limit_node = LimitNode::new(input_node.clone(), offset, count).map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create paging node: {}", e))
        })?;

        Ok(SubPlan::new(
            Some(PlanNodeEnum::Limit(limit_node)),
            input_plan.tail.clone(),
        ))
    }
}

impl ClausePlanner for YieldClausePlanner {
    fn clause_kind(&self) -> CypherClauseKind {
        CypherClauseKind::Yield
    }

    fn transform_clause(
        &self,
        _qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        // Extract the information from the YIELD clause in the sentence.
        let (yield_columns, filter_condition, skip, limit) = Self::extract_yield_info(stmt)?;

        self.plan_yield_clause(&yield_columns, filter_condition, skip, limit, &input_plan)
    }
}

impl YieldClausePlanner {
    /// Extract the information from the YIELD clause in the sentence.
    ///
    /// The improved implementation includes:
    /// Support for the YIELD clause in various statement types
    /// The complete conversion from YieldItem to YieldColumn
    /// Aggregation expression detection
    /// Alias handling
    fn extract_yield_info(stmt: &Stmt) -> YieldInfoResult {
        use crate::query::parser::ast::Stmt;

        // The “YIELD” keyword can appear either as an independent statement or as a clause within other statements.
        match stmt {
            Stmt::Yield(yield_stmt) => {
                let yield_columns = Self::convert_yield_items(&yield_stmt.items)?;
                Ok((yield_columns, yield_stmt.where_clause.clone(), None, None))
            }
            Stmt::Go(go_stmt) => {
                // Extract the YIELD clause from the GO statement.
                if let Some(ref yield_clause) = go_stmt.yield_clause {
                    let yield_columns = Self::convert_yield_items(&yield_clause.items)?;
                    let skip = yield_clause.skip.as_ref().map(|s| s.count);
                    let limit = yield_clause.limit.as_ref().map(|l| l.count);
                    Ok((
                        yield_columns,
                        yield_clause.where_clause.clone(),
                        skip,
                        limit,
                    ))
                } else {
                    Ok((vec![], None, None, None))
                }
            }
            Stmt::Fetch(_fetch_stmt) => {
                // The FETCH statement may have an implicit YIELD.
                Ok((vec![], None, None, None))
            }
            _ => {
                // Other statement types are not currently supported for extracting the YIELD value.
                Ok((vec![], None, None, None))
            }
        }
    }

    /// Convert the YieldItem list to the YieldColumn list.
    fn convert_yield_items(
        items: &[crate::query::parser::ast::stmt::YieldItem],
    ) -> Result<Vec<YieldColumn>, PlannerError> {
        let yield_columns: Vec<YieldColumn> = items
            .iter()
            .map(|item| {
                let alias = item.alias.clone().or_else(|| {
                    if let Some(expr_meta) = item.expression.expression() {
                        Some(Self::generate_default_alias(expr_meta.inner()))
                    } else {
                        Some("expr".to_string())
                    }
                });
                YieldColumn {
                    expression: item.expression.clone(),
                    alias: alias.unwrap_or_else(|| "expr".to_string()),
                    is_matched: false,
                }
            })
            .collect();
        Ok(yield_columns)
    }

    /// Generate default aliases
    ///
    /// When the user does not specify an alias, a default alias is generated based on the expression.
    fn generate_default_alias(expression: &crate::core::Expression) -> String {
        use crate::core::Expression;

        match expression {
            Expression::Variable(name) => name.clone(),
            Expression::Property { object, property } => {
                if let Expression::Variable(name) = object.as_ref() {
                    format!("{}.{}", name, property)
                } else {
                    "expr".to_string()
                }
            }
            Expression::Function { name, .. } => name.clone(),
            Expression::Aggregate { func, .. } => format!("{:?}", func).to_lowercase(),
            _ => "expr".to_string(),
        }
    }
}

impl Default for YieldClausePlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::core::types::ContextualExpression;
    use crate::core::Expression;
    use crate::query::parser::ast::Span;
    use crate::query::planning::plan::core::nodes::StartNode;
    use crate::query::planning::plan::core::PlanNodeEnum;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_yield_clause_planner_creation() {
        let planner = YieldClausePlanner::new();
        assert_eq!(planner.clause_kind(), CypherClauseKind::Yield);
    }

    #[test]
    fn test_extract_yield_info_from_yield_stmt() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let yield_stmt = Stmt::Yield(crate::query::parser::ast::stmt::YieldStmt {
            span: Span::default(),
            items: vec![crate::query::parser::ast::stmt::YieldItem {
                expression: ctx_expr.clone(),
                alias: None,
            }],
            where_clause: None,
            distinct: false,
            order_by: None,
            skip: None,
            limit: None,
        });

        let (columns, filter, skip, limit) =
            YieldClausePlanner::extract_yield_info(&yield_stmt).expect("extract failed");
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].alias, "n");
        assert!(filter.is_none());
        assert!(skip.is_none());
        assert!(limit.is_none());
    }

    #[test]
    fn test_extract_yield_info_from_go_stmt() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let go_stmt = Stmt::Go(crate::query::parser::ast::stmt::GoStmt {
            span: Span::default(),
            steps: crate::query::parser::ast::Steps::Fixed(1),
            from: crate::query::parser::ast::stmt::FromClause {
                span: Span::default(),
                vertices: vec![],
            },
            over: None,
            where_clause: None,
            yield_clause: Some(crate::query::parser::ast::stmt::YieldClause {
                span: Span::default(),
                items: vec![crate::query::parser::ast::stmt::YieldItem {
                    expression: ctx_expr.clone(),
                    alias: None,
                }],
                where_clause: None,
                order_by: None,
                limit: Some(crate::query::parser::ast::types::LimitClause {
                    span: Span::default(),
                    count: 10,
                }),
                skip: Some(crate::query::parser::ast::types::SkipClause {
                    span: Span::default(),
                    count: 5,
                }),
                sample: None,
            }),
        });

        let (columns, filter, skip, limit) =
            YieldClausePlanner::extract_yield_info(&go_stmt).expect("extract failed");
        assert_eq!(columns.len(), 1);
        assert!(filter.is_none());
        assert_eq!(skip, Some(5));
        assert_eq!(limit, Some(10));
    }

    #[test]
    fn test_convert_yield_items() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let items = vec![crate::query::parser::ast::stmt::YieldItem {
            expression: ctx_expr.clone(),
            alias: Some("node".to_string()),
        }];

        let yield_columns =
            YieldClausePlanner::convert_yield_items(&items).expect("convert failed");
        assert_eq!(yield_columns.len(), 1);
        assert_eq!(yield_columns[0].alias, "node");
    }

    #[test]
    fn test_generate_default_alias() {
        let expr = Expression::Variable("n".to_string());
        let alias = YieldClausePlanner::generate_default_alias(&expr);
        assert_eq!(alias, "n");

        let expr = Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "name".to_string(),
        };
        let alias = YieldClausePlanner::generate_default_alias(&expr);
        assert_eq!(alias, "n.name");

        let expr = Expression::Function {
            name: "count".to_string(),
            args: vec![],
        };
        let alias = YieldClausePlanner::generate_default_alias(&expr);
        assert_eq!(alias, "count");
    }

    #[test]
    fn test_transform_clause() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let yield_stmt = Stmt::Yield(crate::query::parser::ast::stmt::YieldStmt {
            span: Span::default(),
            items: vec![crate::query::parser::ast::stmt::YieldItem {
                expression: ctx_expr.clone(),
                alias: None,
            }],
            where_clause: None,
            distinct: false,
            order_by: None,
            skip: None,
            limit: None,
        });

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
        };

        let planner = YieldClausePlanner::new();
        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
            },
        )));

        let result = planner.transform_clause(qctx, &yield_stmt, input_plan);
        assert!(result.is_ok());

        let sub_plan = result.expect("transform_clause should succeed");
        assert!(sub_plan.root.is_some());

        match sub_plan.root {
            Some(PlanNodeEnum::Project(_)) => {}
            Some(PlanNodeEnum::Filter(_)) => {}
            Some(PlanNodeEnum::Limit(_)) => {}
            _ => panic!("Expected ProjectNode, FilterNode, or LimitNode"),
        }
    }

    #[test]
    fn test_transform_clause_empty_input_plan() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("n".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let yield_stmt = Stmt::Yield(crate::query::parser::ast::stmt::YieldStmt {
            span: Span::default(),
            items: vec![crate::query::parser::ast::stmt::YieldItem {
                expression: ctx_expr.clone(),
                alias: None,
            }],
            where_clause: None,
            distinct: false,
            order_by: None,
            skip: None,
            limit: None,
        });

        let input_plan = SubPlan {
            root: None,
            tail: None,
        };

        let planner = YieldClausePlanner::new();
        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
            },
        )));

        let result = planner.transform_clause(qctx, &yield_stmt, input_plan);
        assert!(result.is_err());
    }
}
