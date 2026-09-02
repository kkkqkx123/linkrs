//! Pipe Statement Planner
//!
//! Query planning for handling pipe statements that chain multiple statements together.
//! Supports pipe DELETE syntax: GO ... | DELETE VERTEX $-.id

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::{PipeStmt, Stmt};
use crate::planning::plan::core::nodes::{PipeDeleteEdgesNode, PipeDeleteVerticesNode};
use crate::planning::plan::core::{
    node_id_generator::next_node_id, nodes::base::plan_node_traits::SingleInputNode,
};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerEnum, PlannerError, ValidatedStatement};
use crate::QueryContext;
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
            Arc::new(crate::parser::ast::stmt::Ast::new(
                (*pipe_stmt.left).clone(),
                validated.ast.expr_context().clone(),
            )),
            validated.validation_info.clone(),
        );

        let right_validated = ValidatedStatement::new(
            Arc::new(crate::parser::ast::stmt::Ast::new(
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

        // When a standalone GO (no inline YIELD clause) is the left side of a
        // pipe, the GoPlanner appends a default projection (dst, edge) and a
        // trivial filter (true) to its plan. In a pipe context these are
        // redundant: the pipe stages build their own projections and the
        // filter is a no-op. Elide them so that downstream stages resolve
        // their variables (e.g. target) against the ExpandAll output layout
        // directly.
        let left_root = if matches!(*pipe_stmt.left, Stmt::Go(_)) {
            elide_go_default_adapter(left_root)
        } else {
            left_root
        };

        let combined_root = replace_argument_node(right_root, left_root);

        Ok(SubPlan::new(Some(combined_root), None))
    }

    fn plan_bound(
        &mut self,
        bound: &BoundStatement,
        qctx: Arc<QueryContext>,
        metadata: Option<&crate::metadata::MetadataContext>,
        validated: &ValidatedStatement,
    ) -> Result<SubPlan, PlannerError> {
        let pipe = match bound {
            BoundStatement::Pipe(p) => p,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain the Pipe".to_string(),
                ));
            }
        };

        if pipe.statements.len() < 2 {
            return Err(PlannerError::PlanGenerationFailed(
                "Pipe statement requires at least two sub-statements".to_string(),
            ));
        }

        let mut combined_plan: Option<SubPlan> = None;

        for stmt in &pipe.statements {
            let mut planner = PlannerEnum::from_bound_statement(stmt).ok_or_else(|| {
                PlannerError::NoSuitablePlanner(format!(
                    "No suitable planner for pipe sub-statement: {}",
                    stmt.kind()
                ))
            })?;

            let sub_plan = planner.plan_bound(stmt, qctx.clone(), metadata, validated)?;

            combined_plan = match combined_plan {
                None => Some(sub_plan),
                Some(prev_plan) => {
                    let prev_root = prev_plan.root.ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "Previous pipe stage has no root node".to_string(),
                        )
                    })?;

                    let new_root = sub_plan.root.ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "Current pipe stage has no root node".to_string(),
                        )
                    })?;

                    let combined_root = replace_argument_node(new_root, prev_root);
                    Some(SubPlan::new(Some(combined_root), None))
                }
            };
        }

        combined_plan.ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Pipe statement produced no plan".to_string())
        })
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

/// Strip the default projection (dst, edge) and trivial filter (true) that the
/// GoPlanner attaches to a standalone GO plan. Returns the plan unchanged when
/// its shape does not match the GO default adapter.
fn elide_go_default_adapter(plan: PlanNodeEnum) -> PlanNodeEnum {
    let PlanNodeEnum::Project(project) = plan else {
        return plan;
    };

    let columns = project.columns();
    let is_default_project = columns.len() == 2
        && columns[0].alias == "dst"
        && columns[1].alias == "edge"
        && columns[0].expression.as_variable().as_deref() == Some("dst")
        && columns[1].expression.as_variable().as_deref() == Some("edge");
    if !is_default_project {
        return PlanNodeEnum::Project(project);
    }

    let PlanNodeEnum::Filter(filter) = project.input().clone() else {
        return PlanNodeEnum::Project(project);
    };

    let is_true_filter = filter
        .condition()
        .as_literal()
        .is_some_and(|value| matches!(value, graphdb_core::Value::Bool(true)));
    if !is_true_filter {
        return PlanNodeEnum::Project(project);
    }

    if let PlanNodeEnum::ExpandAll(_) = filter.input().clone() {
        filter.input().clone()
    } else {
        PlanNodeEnum::Project(project)
    }
}

fn replace_argument_node(plan: PlanNodeEnum, replacement: PlanNodeEnum) -> PlanNodeEnum {
    match plan {
        PlanNodeEnum::Argument(_) => replacement,
        PlanNodeEnum::Start(_) => replacement,
        PlanNodeEnum::Project(mut project) => {
            let input = project.input().clone();
            let new_input = replace_argument_node(input, replacement);
            project.set_input(new_input);
            PlanNodeEnum::Project(project)
        }
        PlanNodeEnum::Aggregate(mut aggregate) => {
            // A standalone GROUP BY is planned as Aggregate -> Project -> Scan.
            // When the GROUP BY appears on the right side of a pipe, replace the
            // whole adapter with the left plan so the aggregate consumes the
            // piped rows directly.
            let input = aggregate.input().clone();
            let new_input = match input {
                PlanNodeEnum::Project(mut project) => {
                    if matches!(project.input().clone(), PlanNodeEnum::ScanVertices(_)) {
                        replacement
                    } else {
                        let project_input = project.input().clone();
                        let new_project_input = replace_argument_node(project_input, replacement);
                        project.set_input(new_project_input);
                        PlanNodeEnum::Project(project)
                    }
                }
                other => replace_argument_node(other, replacement),
            };
            aggregate.set_input(new_input);
            PlanNodeEnum::Aggregate(aggregate)
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
