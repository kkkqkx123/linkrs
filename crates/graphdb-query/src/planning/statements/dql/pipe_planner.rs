//! Pipe Statement Planner
//!
//! Query planning for handling pipe statements that chain multiple statements together.
//! Supports pipe DELETE syntax: GO ... | DELETE VERTEX $-.id

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::{PipeStmt, Stmt};
use crate::planning::plan::core::nodes::{PipeDeleteEdgesNode, PipeDeleteVerticesNode};
use crate::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::base::plan_node_traits::{MultipleInputNode, SingleInputNode},
};
use crate::planning::plan::logical::logical_node_traits::LogicalSingleInputNode;
use crate::planning::plan::logical::LogicalNodeEnum;
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

        let left_logical = left_plan.logical_root().cloned();
        let left_root = left_plan.root.ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Left plan has no root node".to_string())
        })?;
        let right_logical = right_plan.logical_root().cloned();
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

        let combined_logical = match (left_logical, right_logical) {
            (Some(left_logical), Some(right_logical)) => {
                let left_logical = if matches!(*pipe_stmt.left, Stmt::Go(_)) {
                    elide_go_default_adapter_logical(left_logical)
                } else {
                    left_logical
                };
                Some(replace_logical_argument(right_logical, left_logical))
            }
            _ => None,
        };

        Ok(SubPlan {
            root: Some(combined_root),
            tail: None,
            logical_root: combined_logical,
        })
    }

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
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

        // The shared `validated` describes the whole composite query, but
        // each stage planner expects the AST fragment aligned with its own
        // bound sub-statement. Derive one per stage up front (owned here so
        // the sub-contexts below can borrow them).
        let mut stage_validated: Vec<ValidatedStatement> =
            Vec::with_capacity(pipe.statements.len());
        for stmt in &pipe.statements {
            stage_validated.push(
                ctx.derive_validated(stmt)
                    .unwrap_or_else(|| ctx.validated.clone()),
            );
        }

        for (stmt, validated) in pipe.statements.iter().zip(stage_validated.iter()) {
            let mut planner = PlannerEnum::from_bound_statement(stmt).ok_or_else(|| {
                PlannerError::NoSuitablePlanner(format!(
                    "No suitable planner for pipe sub-statement: {}",
                    stmt.kind()
                ))
            })?;

            let sub_ctx = crate::planning::context::PlanContext {
                bound: stmt,
                qctx: ctx.qctx.clone(),
                metadata: ctx.metadata,
                validated,
            };
            let sub_plan = planner.plan_bound(&sub_ctx)?;

            combined_plan = match combined_plan {
                None => Some(sub_plan),
                Some(prev_plan) => {
                    let prev_logical = prev_plan.logical_root().cloned();
                    let prev_root = prev_plan.root.ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "Previous pipe stage has no root node".to_string(),
                        )
                    })?;

                    let new_logical = sub_plan.logical_root().cloned();
                    let new_root = sub_plan.root.ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "Current pipe stage has no root node".to_string(),
                        )
                    })?;

                    let combined_root = replace_argument_node(new_root, prev_root);
                    let combined_logical = match (prev_logical, new_logical) {
                        (Some(left_logical), Some(right_logical)) => {
                            Some(replace_logical_argument(right_logical, left_logical))
                        }
                        _ => None,
                    };
                    Some(SubPlan {
                        root: Some(combined_root),
                        tail: None,
                        logical_root: combined_logical,
                    })
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

/// Mirror of [`replace_argument_node`] on the native logical tree: swap the
/// standalone seed (argument/start) of a downstream stage with the upstream
/// logical tree so piped stages keep one chained logical plan.
fn replace_logical_argument(
    plan: LogicalNodeEnum,
    replacement: LogicalNodeEnum,
) -> LogicalNodeEnum {
    match plan {
        LogicalNodeEnum::Argument(_) | LogicalNodeEnum::Start(_) => replacement,
        LogicalNodeEnum::Project(mut project) => {
            if let Some(input) = project.input.take() {
                let new_input = replace_logical_argument(*input, replacement);
                project.set_input(new_input);
            }
            LogicalNodeEnum::Project(project)
        }
        LogicalNodeEnum::Aggregate(mut aggregate) => {
            // Mirror the standalone GROUP BY adapter elision: an aggregate
            // over Project -> ScanVertices consumes the piped rows directly.
            let new_input = match aggregate.input.take() {
                Some(boxed) => match *boxed {
                    LogicalNodeEnum::Project(mut project) => {
                        let is_adapter = matches!(
                            project.input.as_deref(),
                            Some(LogicalNodeEnum::ScanVertices(_))
                        );
                        if is_adapter {
                            replacement
                        } else {
                            if let Some(inner) = project.input.take() {
                                let new_inner = replace_logical_argument(*inner, replacement);
                                project.set_input(new_inner);
                            }
                            LogicalNodeEnum::Project(project)
                        }
                    }
                    other => replace_logical_argument(other, replacement),
                },
                None => replacement,
            };
            aggregate.set_input(new_input);
            LogicalNodeEnum::Aggregate(aggregate)
        }
        LogicalNodeEnum::Filter(mut filter) => {
            if let Some(input) = filter.input.take() {
                let new_input = replace_logical_argument(*input, replacement);
                filter.set_input(new_input);
            }
            LogicalNodeEnum::Filter(filter)
        }
        LogicalNodeEnum::Sort(mut sort) => {
            if let Some(input) = sort.input.take() {
                let new_input = replace_logical_argument(*input, replacement);
                sort.set_input(new_input);
            }
            LogicalNodeEnum::Sort(sort)
        }
        LogicalNodeEnum::Limit(mut limit) => {
            if let Some(input) = limit.input.take() {
                let new_input = replace_logical_argument(*input, replacement);
                limit.set_input(new_input);
            }
            LogicalNodeEnum::Limit(limit)
        }
        LogicalNodeEnum::Dedup(mut dedup) => {
            if let Some(input) = dedup.input.take() {
                let new_input = replace_logical_argument(*input, replacement);
                dedup.set_input(new_input);
            }
            LogicalNodeEnum::Dedup(dedup)
        }
        LogicalNodeEnum::Unwind(mut unwind) => {
            let replacement_cols = replacement.col_names().to_vec();
            if let Some(input) = unwind.input.take() {
                let new_input = replace_logical_argument(*input, replacement);
                unwind.set_input(new_input);
            }
            let mut new_col_names = replacement_cols;
            new_col_names.push(unwind.alias.clone());
            unwind.col_names = new_col_names;
            LogicalNodeEnum::Unwind(unwind)
        }
        LogicalNodeEnum::ExpandAll(mut expand) => {
            expand.deps = expand
                .deps
                .into_iter()
                .map(|d| replace_logical_argument(d, replacement.clone()))
                .collect();
            LogicalNodeEnum::ExpandAll(expand)
        }
        LogicalNodeEnum::GetVertices(mut gv) => {
            gv.deps = gv
                .deps
                .into_iter()
                .map(|d| replace_logical_argument(d, replacement.clone()))
                .collect();
            LogicalNodeEnum::GetVertices(gv)
        }
        other => other,
    }
}

/// Mirror of [`elide_go_default_adapter`] on the native logical tree: strip
/// the default projection and trivial filter of a standalone GO plan when it
/// feeds a pipe, so downstream stages resolve against the expand output.
fn elide_go_default_adapter_logical(plan: LogicalNodeEnum) -> LogicalNodeEnum {
    let LogicalNodeEnum::Project(project) = plan else {
        return plan;
    };

    let columns = &project.columns;
    let is_default_project = columns.len() == 2
        && columns[0].alias == "dst"
        && columns[1].alias == "edge"
        && columns[0].expression.as_variable().as_deref() == Some("dst")
        && columns[1].expression.as_variable().as_deref() == Some("edge");
    if !is_default_project {
        return LogicalNodeEnum::Project(project);
    }

    let Some(input) = project.input.clone() else {
        return LogicalNodeEnum::Project(project);
    };
    let LogicalNodeEnum::Filter(filter) = *input else {
        return LogicalNodeEnum::Project(project);
    };

    let is_true_filter = filter
        .condition
        .as_literal()
        .is_some_and(|value| matches!(value, graphdb_core::Value::Bool(true)));
    if !is_true_filter {
        return LogicalNodeEnum::Project(project);
    }

    match filter.input.clone() {
        Some(boxed) if matches!(*boxed, LogicalNodeEnum::ExpandAll(_)) => *boxed,
        _ => LogicalNodeEnum::Project(project),
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
        PlanNodeEnum::ExpandAll(mut expand) => {
            let new_inputs: Vec<_> = expand
                .inputs()
                .iter()
                .cloned()
                .map(|i| replace_argument_node(i, replacement.clone()))
                .collect();
            expand.inputs_mut().clear();
            for inp in new_inputs {
                expand.add_input(inp);
            }
            PlanNodeEnum::ExpandAll(expand)
        }
        PlanNodeEnum::GetVertices(mut gv) => {
            let new_inputs: Vec<_> = gv
                .inputs()
                .iter()
                .cloned()
                .map(|i| replace_argument_node(i, replacement.clone()))
                .collect();
            gv.inputs_mut().clear();
            for inp in new_inputs {
                gv.add_input(inp);
            }
            PlanNodeEnum::GetVertices(gv)
        }
        other => other,
    }
}
