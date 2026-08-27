//! LIMIT/SKIP Clause Planner
//!
//! Responsible for planning the execution of the LIMIT and SKIP clauses, in order to implement paginated results.

use crate::binder::validation::CypherClauseKind;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::operation::sort_node::LimitNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalLimitNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::planning::statements::match_statement_planner::PaginationInfo;
use crate::planning::statements::plan_combiner::wrap_logical;
use crate::planning::statements::statement_planner::ClausePlanner;
use crate::QueryContext;
use std::sync::Arc;

/// LIMIT/SKIP Clause Planner
///
/// Responsible for planning the execution of the LIMIT and SKIP clauses, in order to implement result pagination.
#[derive(Debug, Default, Clone)]
pub struct PaginationPlanner;

impl PaginationPlanner {
    pub fn new() -> Self {
        Self
    }
}

fn extract_pagination_info(stmt: &Stmt) -> PaginationInfo {
    if let Stmt::Match(match_stmt) = stmt {
        let skip = match_stmt.skip.as_ref().map(|s| s.count).unwrap_or(0);
        let limit = match_stmt.limit.as_ref().map(|l| l.count).unwrap_or(100);
        return PaginationInfo { skip, limit };
    }
    PaginationInfo {
        skip: 0,
        limit: 100,
    }
}

impl ClausePlanner for PaginationPlanner {
    fn clause_kind(&self) -> CypherClauseKind {
        CypherClauseKind::Pagination
    }

    fn transform_clause(
        &self,
        _qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let pagination = extract_pagination_info(stmt);

        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "The LIMIT/SKIP clause requires a schedule entry.".to_string(),
            )
        })?;

        let limit_node = LimitNode::new(
            input_node.clone(),
            pagination.skip as i64,
            pagination.limit as i64,
        )?;
        let logical_root = wrap_logical(&input_plan, |input| {
            LogicalNodeEnum::Limit(LogicalLimitNode {
                id: next_node_id(),
                input: Some(Box::new(input.clone())),
                deps: vec![input],
                offset: pagination.skip as i64,
                count: pagination.limit as i64,
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            })
        });
        Ok(SubPlan {
            root: Some(limit_node.into_enum()),
            tail: input_plan.tail,
            logical_root,
        })
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::parser::ast::types::{LimitClause, SkipClause};
    use crate::parser::ast::Span;
    use crate::planning::plan::core::nodes::StartNode;
    use crate::planning::plan::core::PlanNodeEnum;

    fn limit(count: usize) -> Option<LimitClause> {
        Some(LimitClause {
            count,
            span: Span::default(),
        })
    }

    fn skip(count: usize) -> Option<SkipClause> {
        Some(SkipClause {
            count,
            span: Span::default(),
        })
    }

    #[test]
    fn test_pagination_planner_creation() {
        let planner = PaginationPlanner::new();
        assert_eq!(planner.clause_kind(), CypherClauseKind::Pagination);
    }

    #[test]
    fn test_extract_pagination_info() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: limit(10),
            skip: skip(5),
            optional: false,
            delete_clause: None,
        });

        let pagination = extract_pagination_info(&match_stmt);
        assert_eq!(pagination.skip, 5);
        assert_eq!(pagination.limit, 10);
    }

    #[test]
    fn test_extract_pagination_info_defaults() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let pagination = extract_pagination_info(&match_stmt);
        assert_eq!(pagination.skip, 0);
        assert_eq!(pagination.limit, 100);
    }

    #[test]
    fn test_extract_pagination_info_only_limit() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: limit(20),
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let pagination = extract_pagination_info(&match_stmt);
        assert_eq!(pagination.skip, 0);
        assert_eq!(pagination.limit, 20);
    }

    #[test]
    fn test_extract_pagination_info_only_skip() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: skip(15),
            optional: false,
            delete_clause: None,
        });

        let pagination = extract_pagination_info(&match_stmt);
        assert_eq!(pagination.skip, 15);
        assert_eq!(pagination.limit, 100);
    }

    #[test]
    fn test_transform_clause() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: limit(10),
            skip: skip(5),
            optional: false,
            delete_clause: None,
        });

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
            logical_root: None,
        };

        let planner = PaginationPlanner::new();
        let qctx = std::sync::Arc::new(crate::QueryContext::new(std::sync::Arc::new(
            crate::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_ok());

        let sub_plan = result.expect("transform_clause should succeed");
        assert!(sub_plan.root.is_some());

        if let Some(PlanNodeEnum::Limit(_)) = sub_plan.root {
        } else {
            panic!("Expected LimitNode");
        }
    }

    #[test]
    fn test_transform_clause_defaults() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
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
            logical_root: None,
        };

        let planner = PaginationPlanner::new();
        let qctx = std::sync::Arc::new(crate::QueryContext::new(std::sync::Arc::new(
            crate::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_ok());

        let sub_plan = result.expect("transform_clause should succeed");
        assert!(sub_plan.root.is_some());

        if let Some(PlanNodeEnum::Limit(_)) = sub_plan.root {
        } else {
            panic!("Expected LimitNode");
        }
    }

    #[test]
    fn test_transform_clause_empty_input_plan() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: limit(10),
            skip: skip(5),
            optional: false,
            delete_clause: None,
        });

        let input_plan = SubPlan {
            root: None,
            tail: None,
            logical_root: None,
        };

        let planner = PaginationPlanner::new();
        let qctx = std::sync::Arc::new(crate::QueryContext::new(std::sync::Arc::new(
            crate::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_err());
    }
}
