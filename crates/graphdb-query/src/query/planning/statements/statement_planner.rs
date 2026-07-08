//! Statement-level planner
//!
//! Provide a unified interface for statement-level planners that handles the planning logic of entire statements.
//! Architecture: Planner trait -> StatementPlanner trait -> ClausePlanner
//!
//! ## Architecture Design
//!
//! **Planner**: A basic trait that defines the common interface for planners.
//! **StatementPlanner**: A trait at the statement level, responsible for the planning of entire sentences.
//! **ClausePlanner**: A trait at the clause level, responsible for the planning of individual clauses.

use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::Planner;
use crate::query::validator::structs::CypherClauseKind;
use crate::query::QueryContext;
use std::sync::Arc;

/// Statement-level planner trait
///
/// Define a unified interface for statement-level planners that encapsulates the entire planning logic for processing statements.
/// Use a combination of multiple sub-phrase planners to complete the planning of the sentence.
pub trait StatementPlanner: Planner {
    /// Determine the type of the statement
    fn statement_type(&self) -> &'static str;

    /// Obtain a list of the supported clause types.
    fn supported_clause_kinds(&self) -> &[CypherClauseKind];
}

/// Clause-level planner trait
///
/// Define a unified interface for clause-level planners that handles the planning logic of individual clauses.
pub trait ClausePlanner: std::fmt::Debug {
    /// Determine the type of the clause.
    fn clause_kind(&self) -> CypherClauseKind;

    /// Turn the sentence into the core plan.
    fn transform_clause(
        &self,
        qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, crate::query::planning::planner::PlannerError>;
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::query::parser::ast::{Ast, Span};
    use crate::query::planning::plan::core::nodes::StartNode;
    use crate::query::planning::plan::core::PlanNodeEnum;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::query::validator::ValidatedStatement;
    use crate::query::QueryRequestContext;
    use std::collections::HashMap;

    #[derive(Debug)]
    struct MockStatementPlanner {
        stmt_type: &'static str,
        supported_kinds: Vec<CypherClauseKind>,
    }

    impl MockStatementPlanner {
        fn new(stmt_type: &'static str, supported_kinds: Vec<CypherClauseKind>) -> Self {
            Self {
                stmt_type,
                supported_kinds,
            }
        }
    }

    impl Planner for MockStatementPlanner {
        fn transform(
            &mut self,
            _validated: &ValidatedStatement,
            _qctx: Arc<QueryContext>,
        ) -> Result<SubPlan, crate::query::planning::planner::PlannerError> {
            let start_node = StartNode::new();
            let start_node_enum = PlanNodeEnum::Start(start_node);
            Ok(SubPlan {
                root: Some(start_node_enum.clone()),
                tail: Some(start_node_enum),
            })
        }

        fn match_planner(&self, _stmt: &Stmt) -> bool {
            true
        }
    }

    impl StatementPlanner for MockStatementPlanner {
        fn statement_type(&self) -> &'static str {
            self.stmt_type
        }

        fn supported_clause_kinds(&self) -> &[CypherClauseKind] {
            &self.supported_kinds
        }
    }

    #[derive(Debug)]
    struct MockClausePlanner {
        kind: CypherClauseKind,
    }

    impl MockClausePlanner {
        fn new(kind: CypherClauseKind) -> Self {
            Self { kind }
        }
    }

    impl ClausePlanner for MockClausePlanner {
        fn clause_kind(&self) -> CypherClauseKind {
            self.kind
        }

        fn transform_clause(
            &self,
            _qctx: Arc<QueryContext>,
            _stmt: &Stmt,
            input_plan: SubPlan,
        ) -> Result<SubPlan, crate::query::planning::planner::PlannerError> {
            Ok(input_plan)
        }
    }

    fn create_test_qctx() -> Arc<QueryContext> {
        let rctx = Arc::new(QueryRequestContext {
            session_id: None,
            user_name: None,
            space_name: None,
            query: String::new(),
            parameters: HashMap::new(),
        });
        Arc::new(QueryContext::new(rctx))
    }

    fn create_test_match_stmt() -> Arc<Ast> {
        let stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
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
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        Arc::new(Ast::new(stmt, ctx))
    }

    #[test]
    fn test_statement_planner_statement_type() {
        let planner = MockStatementPlanner::new(
            "MATCH",
            vec![CypherClauseKind::Match, CypherClauseKind::Where],
        );
        assert_eq!(planner.statement_type(), "MATCH");
    }

    #[test]
    fn test_statement_planner_supported_clause_kinds() {
        let supported_kinds = vec![CypherClauseKind::Match, CypherClauseKind::Where];
        let planner = MockStatementPlanner::new("MATCH", supported_kinds.clone());
        assert_eq!(planner.supported_clause_kinds(), &supported_kinds);
    }

    #[test]
    fn test_statement_planner_transform() {
        use crate::query::validator::ValidationInfo;

        let mut planner = MockStatementPlanner::new("MATCH", vec![CypherClauseKind::Match]);
        let ast = create_test_match_stmt();
        let qctx = create_test_qctx();

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(result.is_ok());
        let sub_plan = result.expect("transform should succeed");
        assert!(sub_plan.root.is_some());
        assert!(sub_plan.tail.is_some());
    }

    #[test]
    fn test_statement_planner_match_planner() {
        let planner = MockStatementPlanner::new("MATCH", vec![CypherClauseKind::Match]);
        let ast = create_test_match_stmt();
        assert!(planner.match_planner(&ast.stmt));
    }

    #[test]
    fn test_clause_planner_clause_kind() {
        let planner = MockClausePlanner::new(CypherClauseKind::Where);
        assert_eq!(planner.clause_kind(), CypherClauseKind::Where);
    }

    #[test]
    fn test_clause_planner_transform_clause() {
        let planner = MockClausePlanner::new(CypherClauseKind::Where);
        let qctx = create_test_qctx();
        let ast = create_test_match_stmt();

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
        };

        let result = planner.transform_clause(qctx, &ast.stmt, input_plan);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiple_supported_clause_kinds() {
        let supported_kinds = vec![
            CypherClauseKind::Match,
            CypherClauseKind::Where,
            CypherClauseKind::Return,
            CypherClauseKind::With,
        ];
        let planner = MockStatementPlanner::new("MATCH", supported_kinds.clone());
        assert_eq!(planner.supported_clause_kinds().len(), 4);
        assert!(planner
            .supported_clause_kinds()
            .contains(&CypherClauseKind::Match));
        assert!(planner
            .supported_clause_kinds()
            .contains(&CypherClauseKind::Where));
        assert!(planner
            .supported_clause_kinds()
            .contains(&CypherClauseKind::Return));
        assert!(planner
            .supported_clause_kinds()
            .contains(&CypherClauseKind::With));
    }

    #[test]
    fn test_clause_planner_different_kinds() {
        let where_planner = MockClausePlanner::new(CypherClauseKind::Where);
        let return_planner = MockClausePlanner::new(CypherClauseKind::Return);
        let with_planner = MockClausePlanner::new(CypherClauseKind::With);

        assert_eq!(where_planner.clause_kind(), CypherClauseKind::Where);
        assert_eq!(return_planner.clause_kind(), CypherClauseKind::Return);
        assert_eq!(with_planner.clause_kind(), CypherClauseKind::With);
    }
}
