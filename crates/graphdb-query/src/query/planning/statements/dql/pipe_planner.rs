//! Pipe Statement Planner
//!
//! Query planning for handling pipe statements that chain multiple statements together.
//! Supports pipe DELETE syntax: GO ... | DELETE VERTEX $-.id

use crate::query::parser::ast::stmt::{PipeStmt, Stmt};
use crate::query::planning::plan::core::nodes::{PipeDeleteEdgesNode, PipeDeleteVerticesNode};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id, nodes::base::plan_node_traits::SingleInputNode,
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerEnum, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

/// Pipe Statement Planner
/// Responsible for converting pipe statements into execution plans.
#[derive(Debug, Clone)]
pub struct PipePlanner;

impl PipePlanner {
    pub fn new() -> Self {
        Self
    }

    fn extract_pipe_stmt(&self, stmt: &Stmt) -> Result<PipeStmt, PlannerError> {
        match stmt {
            Stmt::Pipe(pipe_stmt) => Ok(pipe_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain the Pipe".to_string(),
            )),
        }
    }
}

impl Planner for PipePlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let pipe_stmt = self.extract_pipe_stmt(validated.stmt())?;

        let left_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (*pipe_stmt.left).clone(),
                validated.ast.expr_context().clone(),
            )),
            validated.validation_info.clone(),
        );

        let right_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (*pipe_stmt.right).clone(),
                validated.ast.expr_context().clone(),
            )),
            validated.validation_info.clone(),
        );

        let mut left_planner = PlannerEnum::from_stmt(&Arc::new((*pipe_stmt.left).clone()))
            .ok_or_else(|| PlannerError::NoSuitablePlanner("left statement".to_string()))?;
        let left_plan = left_planner.transform(&left_validated, qctx.clone())?;

        let mut right_planner = PlannerEnum::from_stmt(&Arc::new((*pipe_stmt.right).clone()))
            .ok_or_else(|| PlannerError::NoSuitablePlanner("right statement".to_string()))?;

        let right_plan = right_planner.transform(&right_validated, qctx)?;

        let left_root = left_plan.root.ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Left plan has no root node".to_string())
        })?;
        let right_root = right_plan.root.ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Right plan has no root node".to_string())
        })?;

        let combined_root = replace_argument_node(right_root, left_root);

        Ok(SubPlan::new(Some(combined_root), None))
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Pipe(_))
    }
}

impl Default for PipePlanner {
    fn default() -> Self {
        Self::new()
    }
}

fn replace_argument_node(plan: PlanNodeEnum, replacement: PlanNodeEnum) -> PlanNodeEnum {
    match plan {
        PlanNodeEnum::Argument(_) => replacement,
        PlanNodeEnum::Project(mut project) => {
            let input = project.input().clone();
            let new_input = replace_argument_node(input, replacement);
            project.set_input(new_input);
            PlanNodeEnum::Project(project)
        }
        PlanNodeEnum::Filter(mut filter) => {
            let input = filter.input().clone();
            let new_input = replace_argument_node(input, replacement);
            filter.set_input(new_input);
            PlanNodeEnum::Filter(filter)
        }
        PlanNodeEnum::Sort(mut sort) => {
            let input = sort.input().clone();
            let new_input = replace_argument_node(input, replacement);
            sort.set_input(new_input);
            PlanNodeEnum::Sort(sort)
        }
        PlanNodeEnum::Limit(mut limit) => {
            let input = limit.input().clone();
            let new_input = replace_argument_node(input, replacement);
            limit.set_input(new_input);
            PlanNodeEnum::Limit(limit)
        }
        PlanNodeEnum::Dedup(mut dedup) => {
            let input = dedup.input().clone();
            let new_input = replace_argument_node(input, replacement);
            dedup.set_input(new_input);
            PlanNodeEnum::Dedup(dedup)
        }
        PlanNodeEnum::Unwind(mut unwind) => {
            let input = unwind.input().clone();
            let new_input = replace_argument_node(input, replacement.clone());
            unwind.set_input(new_input);

            let mut new_col_names = replacement.col_names().to_vec();
            if let Some(alias) = unwind.col_names().last() {
                new_col_names.push(alias.clone());
            }
            unwind.set_col_names(new_col_names);

            PlanNodeEnum::Unwind(unwind)
        }
        PlanNodeEnum::DeleteVertices(delete_vertices) => {
            let info = delete_vertices.info().clone();
            let node = PipeDeleteVerticesNode::new(next_node_id(), info, replacement);
            PlanNodeEnum::PipeDeleteVertices(node)
        }
        PlanNodeEnum::DeleteEdges(delete_edges) => {
            let info = delete_edges.info().clone();
            let node = PipeDeleteEdgesNode::new(next_node_id(), info, replacement);
            PlanNodeEnum::PipeDeleteEdges(node)
        }
        PlanNodeEnum::PipeDeleteVertices(mut pipe_delete_vertices) => {
            let input = pipe_delete_vertices.input().clone();
            let new_input = replace_argument_node(input, replacement);
            pipe_delete_vertices.set_input(new_input);
            PlanNodeEnum::PipeDeleteVertices(pipe_delete_vertices)
        }
        PlanNodeEnum::PipeDeleteEdges(mut pipe_delete_edges) => {
            let input = pipe_delete_edges.input().clone();
            let new_input = replace_argument_node(input, replacement);
            pipe_delete_edges.set_input(new_input);
            PlanNodeEnum::PipeDeleteEdges(pipe_delete_edges)
        }
        other => other,
    }
}
