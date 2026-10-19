//! COPY FROM Planner
//!
//! Plans COPY VERTEX/EDGE FROM CSV statements into CopyFromNode
//!
//! Migrated to generate a native LogicalNodeEnum tree; `from_logical_root`
//! performs the one-shot logical → physical lowering so the optimizer sees
//! the logical mirror.

use crate::binder::BoundStatement;
use crate::parser::ast::{CopyDirection, CopyTarget as AstCopyTarget, Stmt};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::CopyTarget;
use crate::planning::plan::logical::logical_nodes::dml::{LogicalCopyFromNode, LogicalCopyToNode};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};

#[derive(Debug, Clone)]
pub struct CopyPlanner;

impl CopyPlanner {
    pub fn new() -> Self {
        Self
    }

    pub fn match_stmt(stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Copy(_))
    }
}

impl Planner for CopyPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let copy = match bound {
            BoundStatement::Copy(c) => c,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement is not a COPY statement".to_string(),
                ));
            }
        };

        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        let target = match &copy.target {
            AstCopyTarget::Vertex(tag) => CopyTarget::Vertex(tag.clone()),
            AstCopyTarget::Edge(edge) => CopyTarget::Edge(edge.clone()),
        };

        let batch_size = copy.batch_size.unwrap_or(1000);
        if batch_size == 0 {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY batch_size must be > 0".to_string(),
            ));
        }

        if copy.file_paths.is_empty() || copy.file_paths.iter().any(|p| p.trim().is_empty()) {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY file path must not be empty".to_string(),
            ));
        }
        if copy.direction == CopyDirection::To && copy.file_paths.len() > 1 {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY TO supports a single file path".to_string(),
            ));
        }

        let logical_root = match copy.direction {
            CopyDirection::From => LogicalNodeEnum::CopyFrom(LogicalCopyFromNode {
                id: next_node_id(),
                space_name,
                target,
                file_paths: copy.file_paths.clone(),
                by_column: copy.by_column,
                header: copy.header,
                delimiter: copy.delimiter,
                batch_size,
                output_var: None,
                col_names: vec!["copy_result".to_string()],
                column_types: vec![],
            }),
            CopyDirection::To => LogicalNodeEnum::CopyTo(LogicalCopyToNode {
                id: next_node_id(),
                space_name,
                target,
                file_path: copy.file_paths.first().cloned().unwrap_or_default(),
                header: copy.header,
                delimiter: copy.delimiter,
                output_var: None,
                col_names: vec!["copy_result".to_string()],
                column_types: vec![],
            }),
        };

        Ok(SubPlan::from_logical_root(logical_root))
    }


}

impl Default for CopyPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::QueryContext;
    use crate::parser::ast::{Ast, CopyStmt, CopyTarget as AstCopyTarget};
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::Span;
    use std::sync::Arc;

    fn create_copy_stmt(target: AstCopyTarget) -> Ast {
        let stmt = Stmt::Copy(CopyStmt {
            span: Span::default(),
            target,
            direction: CopyDirection::From,
            file_paths: vec!["data.csv".to_string()],
            by_column: false,
            header: true,
            delimiter: ',',
            batch_size: Some(100),
        });
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        Ast::new(stmt, ctx)
    }

    #[test]
    fn test_copy_planner_match() {
        let ast = create_copy_stmt(AstCopyTarget::Vertex("person".to_string()));
        let planner = CopyPlanner::new();
        assert_eq!(planner.name(), std::any::type_name::<CopyPlanner>());
        assert!(CopyPlanner::match_stmt(&ast.stmt));
    }

    #[test]
    fn test_copy_planner_plans_vertex() {
        let mut planner = CopyPlanner::new();
        let ast = Arc::new(create_copy_stmt(AstCopyTarget::Vertex(
            "person".to_string(),
        )));
        let qctx = Arc::new(QueryContext::default());
        let bound = crate::binder::Binder::new()
            .with_space(qctx.space_name(), qctx.space_id().unwrap_or(0))
            .bind(ast)
            .expect("copy statement binds");
        let plan_ctx = crate::planning::context::PlanContext::without_metadata(&bound, qctx);
        let plan = planner.plan_bound(&plan_ctx).expect("copy plan");
        assert!(plan.root.is_some());
        assert!(plan.root.as_ref().unwrap().is_copy_from());
    }

    #[test]
    fn test_copy_planner_plans_edge() {
        let mut planner = CopyPlanner::new();
        let ast = Arc::new(create_copy_stmt(AstCopyTarget::Edge("knows".to_string())));
        let qctx = Arc::new(QueryContext::default());
        let bound = crate::binder::Binder::new()
            .with_space(qctx.space_name(), qctx.space_id().unwrap_or(0))
            .bind(ast)
            .expect("copy statement binds");
        let plan_ctx = crate::planning::context::PlanContext::without_metadata(&bound, qctx);
        let plan = planner.plan_bound(&plan_ctx).expect("copy plan");
        assert!(plan.root.is_some());
    }
}
