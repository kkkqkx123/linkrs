//! COPY FROM Planner
//!
//! Plans COPY VERTEX/EDGE FROM CSV statements into CopyFromNode

use std::sync::Arc;

use crate::binder::BoundStatement;
use crate::parser::ast::{CopyDirection, CopyStmt, CopyTarget as AstCopyTarget, Stmt};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::ArgumentNode;
use crate::planning::plan::core::nodes::{CopyFromNode, CopyTarget as PlanCopyTarget, CopyToNode};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;

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
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
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
            AstCopyTarget::Vertex(tag) => PlanCopyTarget::Vertex(tag.clone()),
            AstCopyTarget::Edge(edge) => PlanCopyTarget::Edge(edge.clone()),
        };

        let batch_size = copy.batch_size.unwrap_or(1000);
        if batch_size == 0 {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY batch_size must be > 0".to_string(),
            ));
        }

        if copy.file_path.trim().is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY file path must not be empty".to_string(),
            ));
        }

        let arg_node = || ArgumentNode::new(next_node_id(), "copy_args");
        match copy.direction {
            CopyDirection::From => {
                let node = CopyFromNode::new(
                    next_node_id(),
                    space_name,
                    target,
                    copy.file_path.clone(),
                    copy.header,
                    copy.delimiter,
                    batch_size,
                );
                Ok(SubPlan::new(
                    Some(PlanNodeEnum::CopyFrom(node)),
                    Some(PlanNodeEnum::Argument(arg_node())),
                ))
            }
            CopyDirection::To => {
                let node = CopyToNode::new(
                    next_node_id(),
                    space_name,
                    target,
                    copy.file_path.clone(),
                    copy.header,
                    copy.delimiter,
                );
                Ok(SubPlan::new(
                    Some(PlanNodeEnum::CopyTo(node)),
                    Some(PlanNodeEnum::Argument(arg_node())),
                ))
            }
        }
    }

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

        let batch_size = copy_stmt.batch_size.unwrap_or(1000);
        if batch_size == 0 {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY batch_size must be > 0".to_string(),
            ));
        }

        // Validate file path is not empty (both directions).
        if copy_stmt.file_path.trim().is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "COPY file path must not be empty".to_string(),
            ));
        }

        let arg_node = || ArgumentNode::new(next_node_id(), "copy_args");
        match copy_stmt.direction {
            CopyDirection::From => {
                let node = CopyFromNode::new(
                    next_node_id(),
                    space_name,
                    target,
                    copy_stmt.file_path,
                    copy_stmt.header,
                    copy_stmt.delimiter,
                    batch_size,
                );
                Ok(SubPlan::new(
                    Some(PlanNodeEnum::CopyFrom(node)),
                    Some(PlanNodeEnum::Argument(arg_node())),
                ))
            }
            CopyDirection::To => {
                let node = CopyToNode::new(
                    next_node_id(),
                    space_name,
                    target,
                    copy_stmt.file_path,
                    copy_stmt.header,
                    copy_stmt.delimiter,
                );
                Ok(SubPlan::new(
                    Some(PlanNodeEnum::CopyTo(node)),
                    Some(PlanNodeEnum::Argument(arg_node())),
                ))
            }
        }
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
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::binder::validation::ValidationInfo;
    use crate::parser::ast::{Ast, CopyStmt, CopyTarget as AstCopyTarget};
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::Span;

    fn create_copy_stmt(target: AstCopyTarget) -> Ast {
        let stmt = Stmt::Copy(CopyStmt {
            span: Span::default(),
            target,
            direction: CopyDirection::From,
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
