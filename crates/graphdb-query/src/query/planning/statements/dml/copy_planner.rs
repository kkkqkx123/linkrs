//! COPY FROM Planner
//!
//! Plans COPY VERTEX/EDGE FROM CSV statements into CopyFromNode

use std::sync::Arc;

use crate::query::parser::ast::{CopyStmt, CopyTarget as AstCopyTarget, Stmt};
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::ArgumentNode;
use crate::query::planning::plan::core::nodes::{CopyFromNode, CopyTarget as PlanCopyTarget};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;

#[derive(Debug, Clone)]
pub struct CopyPlanner;

impl CopyPlanner {
    pub fn new() -> Self {
        Self
    }

    pub fn match_stmt(stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Copy(_))
    }

    fn extract_copy_stmt(&self, stmt: &Stmt) -> Result<CopyStmt, PlannerError> {
        match stmt {
            Stmt::Copy(copy) => Ok(copy.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement is not a COPY statement".to_string(),
            )),
        }
    }
}

impl Planner for CopyPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let copy_stmt = self.extract_copy_stmt(validated.stmt())?;

        let target = match copy_stmt.target {
            AstCopyTarget::Vertex(tag) => PlanCopyTarget::Vertex(tag),
            AstCopyTarget::Edge(edge) => PlanCopyTarget::Edge(edge),
        };

        // Validate file path is not empty
        if copy_stmt.file_path.trim().is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY file path must not be empty".to_string(),
            ));
        }

        let batch_size = copy_stmt.batch_size.unwrap_or(1000);
        if batch_size == 0 {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY batch_size must be > 0".to_string(),
            ));
        }

        let node = CopyFromNode::new(
            next_node_id(),
            space_name,
            target,
            copy_stmt.file_path,
            copy_stmt.header,
            copy_stmt.delimiter,
            batch_size,
        );

        let arg_node = ArgumentNode::new(next_node_id(), "copy_args");
        let sub_plan = SubPlan::new(
            Some(PlanNodeEnum::CopyFrom(node)),
            Some(PlanNodeEnum::Argument(arg_node)),
        );
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        Self::match_stmt(stmt)
    }
}

impl Default for CopyPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::Span;
    use crate::query::binder::validation::ValidationInfo;
    use crate::query::parser::ast::{Ast, CopyStmt, CopyTarget as AstCopyTarget};

    fn create_copy_stmt(target: AstCopyTarget) -> Ast {
        let stmt = Stmt::Copy(CopyStmt {
            span: Span::default(),
            target,
            file_path: "data.csv".to_string(),
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
        assert!(planner.match_planner(&ast.stmt));
    }

    #[test]
    fn test_copy_planner_transform_vertex() {
        let mut planner = CopyPlanner::new();
        let ast = Arc::new(create_copy_stmt(AstCopyTarget::Vertex(
            "person".to_string(),
        )));
        let qctx = Arc::new(QueryContext::default());
        let validation = ValidationInfo::new();
        let validated = ValidatedStatement::new(ast, validation);
        let plan = planner.transform(&validated, qctx).expect("copy plan");
        assert!(plan.root.is_some());
        assert!(plan.root.as_ref().unwrap().is_copy_from());
    }

    #[test]
    fn test_copy_planner_transform_edge() {
        let mut planner = CopyPlanner::new();
        let ast = Arc::new(create_copy_stmt(AstCopyTarget::Edge("knows".to_string())));
        let qctx = Arc::new(QueryContext::default());
        let validation = ValidationInfo::new();
        let validated = ValidatedStatement::new(ast, validation);
        let plan = planner.transform(&validated, qctx).expect("copy plan");
        assert!(plan.root.is_some());
    }
}
