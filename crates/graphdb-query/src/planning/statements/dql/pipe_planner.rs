//! Pipe Statement Planner
//!
//! Query planning for handling pipe statements that chain multiple statements together.
//! Supports pipe DELETE syntax: GO ... | DELETE VERTEX $-.id

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::{PipeStmt, Stmt};
use crate::planning::plan::core::nodes::{PipeDeleteEdgesNode, PipeDeleteVerticesNode, StartNode};
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

    fn extract_pipe_stmt<'a>(&self, stmt: &'a Stmt) -> Result<&'a PipeStmt, PlannerError> {
        match stmt {
            Stmt::Pipe(pipe_stmt) => Ok(pipe_stmt),
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

        // Clone each branch once for its sub-context; planner dispatch below
        // reuses those owned fragments by reference instead of cloning again.
        let left_stmt = (*pipe_stmt.left).clone();
        let right_stmt = (*pipe_stmt.right).clone();

        let left_validated = ValidatedStatement::new(
            Arc::new(crate::parser::ast::stmt::Ast::new(
                left_stmt,
                validated.ast.expr_context().clone(),
            )),
            validated.validation_info.clone(),
        );

        let right_validated = ValidatedStatement::new(
            Arc::new(crate::parser::ast::stmt::Ast::new(
                right_stmt,
                validated.ast.expr_context().clone(),
            )),
            validated.validation_info.clone(),
        );

        let mut left_planner = PlannerEnum::from_stmt_ref(left_validated.stmt())
            .ok_or_else(|| PlannerError::NoSuitablePlanner("left statement".to_string()))?;
        let mut left_plan = left_planner.transform(&left_validated, qctx.clone())?;

        let mut right_planner = PlannerEnum::from_stmt_ref(right_validated.stmt())
            .ok_or_else(|| PlannerError::NoSuitablePlanner("right statement".to_string()))?;

        let mut right_plan = right_planner.transform(&right_validated, qctx)?;

        let left_logical = left_plan.logical_root.take();
        let left_root = left_plan.root.take().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("Left plan has no root node".to_string())
        })?;
        let right_logical = right_plan.logical_root.take();
        let right_root = right_plan.root.take().ok_or_else(|| {
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

        let mut combined_root = right_root;
        replace_argument_node(&mut combined_root, &left_root);

        let combined_logical = match (left_logical, right_logical) {
            (Some(left_logical), Some(right_logical)) => {
                let left_logical = if matches!(*pipe_stmt.left, Stmt::Go(_)) {
                    elide_go_default_adapter_logical(left_logical)
                } else {
                    left_logical
                };
                let mut combined = right_logical;
                replace_logical_argument(&mut combined, &left_logical);
                Some(combined)
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
                    let mut prev_plan = prev_plan;
                    let mut sub_plan = sub_plan;
                    let prev_logical = prev_plan.logical_root.take();
                    let prev_root = prev_plan.root.take().ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "Previous pipe stage has no root node".to_string(),
                        )
                    })?;

                    let new_logical = sub_plan.logical_root.take();
                    let new_root = sub_plan.root.take().ok_or_else(|| {
                        PlannerError::PlanGenerationFailed(
                            "Current pipe stage has no root node".to_string(),
                        )
                    })?;

                    // Mirror the legacy transform path: a leading GO without
                    // an explicit YIELD carries a default Project(dst, edge)
                    // adapter that drops the ExpandAll `target` alias. Elide
                    // it so downstream stages resolve `target` directly.
                    let prev_root = elide_go_default_adapter(prev_root);
                    let prev_logical = prev_logical.map(elide_go_default_adapter_logical);

                    let mut combined_root = new_root;
                    replace_argument_node(&mut combined_root, &prev_root);
                    let combined_logical = match (prev_logical, new_logical) {
                        (Some(left_logical), Some(right_logical)) => {
                            let mut combined = right_logical;
                            replace_logical_argument(&mut combined, &left_logical);
                            Some(combined)
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

/// Strip the default projection (dst, edge) that the GoPlanner attaches to
/// a standalone GO plan. Handles both the legacy shape with a trivial
/// filter (Project -> Filter(true) -> ExpandAll) and the bound shape without
/// it (Project -> ExpandAll). Returns the plan unchanged when its shape does
/// not match the GO default adapter.
fn elide_go_default_adapter(plan: PlanNodeEnum) -> PlanNodeEnum {
    let PlanNodeEnum::Project(mut project) = plan else {
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

    // Take the input by value instead of cloning the whole subtree; the
    // detached inputs below are restored when the shape does not match.
    let input = take_single_input(project.input_mut());
    match input {
        // Bound path: Project directly over ExpandAll.
        expand @ PlanNodeEnum::ExpandAll(_) => expand,
        PlanNodeEnum::Filter(mut filter) => {
            let is_true_filter = filter
                .condition()
                .as_literal()
                .is_some_and(|value| matches!(value, graphdb_core::Value::Bool(true)));
            if !is_true_filter {
                project.set_input(PlanNodeEnum::Filter(filter));
                return PlanNodeEnum::Project(project);
            }
            let inner = take_single_input(filter.input_mut());
            match inner {
                expand @ PlanNodeEnum::ExpandAll(_) => expand,
                other => {
                    filter.set_input(other);
                    project.set_input(PlanNodeEnum::Filter(filter));
                    PlanNodeEnum::Project(project)
                }
            }
        }
        other => {
            project.set_input(other);
            PlanNodeEnum::Project(project)
        }
    }
}

/// Mirror of [`replace_argument_node`] on the native logical tree: swap the
/// standalone seed (argument/start) of a downstream stage with the upstream
/// logical tree so piped stages keep one chained logical plan.
///
/// Rewrites in place through `&mut` so deep pipe chains only hold two
/// pointers per recursion level instead of two owned trees.
fn replace_logical_argument(plan: &mut LogicalNodeEnum, replacement: &LogicalNodeEnum) {
    use crate::planning::plan::logical::logical_node_traits::{
        LogicalMultipleInputNode, LogicalSingleInputNode,
    };
    if matches!(
        plan,
        LogicalNodeEnum::Argument(_) | LogicalNodeEnum::Start(_)
    ) {
        *plan = replacement.clone();
        return;
    }
    match plan {
        LogicalNodeEnum::Project(project) => {
            if let Some(mut input) = project.input.take() {
                replace_logical_argument(&mut input, replacement);
                project.set_input(*input);
            }
        }
        LogicalNodeEnum::Aggregate(aggregate) => {
            // Mirror the standalone GROUP BY adapter elision: an aggregate
            // over Project -> ScanVertices consumes the piped rows directly.
            let taken = aggregate.input.take();
            let new_input = match taken {
                Some(boxed) => match *boxed {
                    LogicalNodeEnum::Project(mut project) => {
                        let is_adapter = matches!(
                            project.input.as_deref(),
                            Some(LogicalNodeEnum::ScanVertices(_))
                        );
                        if is_adapter {
                            replacement.clone()
                        } else {
                            if let Some(mut inner) = project.input.take() {
                                replace_logical_argument(&mut inner, replacement);
                                project.set_input(*inner);
                            }
                            LogicalNodeEnum::Project(project)
                        }
                    }
                    mut other => {
                        replace_logical_argument(&mut other, replacement);
                        other
                    }
                },
                None => replacement.clone(),
            };
            aggregate.set_input(new_input);
        }
        LogicalNodeEnum::Filter(filter) => {
            if let Some(mut input) = filter.input.take() {
                replace_logical_argument(&mut input, replacement);
                filter.set_input(*input);
            }
        }
        LogicalNodeEnum::Sort(sort) => {
            if let Some(mut input) = sort.input.take() {
                replace_logical_argument(&mut input, replacement);
                sort.set_input(*input);
            }
        }
        LogicalNodeEnum::Limit(limit) => {
            if let Some(mut input) = limit.input.take() {
                replace_logical_argument(&mut input, replacement);
                limit.set_input(*input);
            }
        }
        LogicalNodeEnum::Dedup(dedup) => {
            if let Some(mut input) = dedup.input.take() {
                replace_logical_argument(&mut input, replacement);
                dedup.set_input(*input);
            }
        }
        LogicalNodeEnum::Unwind(unwind) => {
            let replacement_cols = replacement.col_names().to_vec();
            if let Some(mut input) = unwind.input.take() {
                replace_logical_argument(&mut input, replacement);
                unwind.set_input(*input);
            }
            let mut new_col_names = replacement_cols;
            new_col_names.push(unwind.alias.clone());
            unwind.col_names = new_col_names;
        }
        LogicalNodeEnum::ExpandAll(expand) => {
            for dep in expand.inputs_mut() {
                replace_logical_argument(dep, replacement);
            }
        }
        LogicalNodeEnum::GetVertices(gv) => {
            for dep in gv.inputs_mut() {
                replace_logical_argument(dep, replacement);
            }
        }
        _ => {}
    }
}

/// Mirror of [`elide_go_default_adapter`] on the native logical tree: strip
/// the default projection of a standalone GO plan when it feeds a pipe, so
/// downstream stages resolve against the expand output. Handles both the
/// legacy Project -> Filter(true) -> ExpandAll shape and the bound
/// Project -> ExpandAll shape.
fn elide_go_default_adapter_logical(plan: LogicalNodeEnum) -> LogicalNodeEnum {
    let LogicalNodeEnum::Project(mut project) = plan else {
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

    // Judge by reference first so a mismatch costs no clone.
    if matches!(
        project.input.as_deref(),
        Some(LogicalNodeEnum::ExpandAll(_))
    ) {
        return *project.input.take().expect("checked project input");
    }
    let input = match project.input.take() {
        Some(boxed) => *boxed,
        None => return LogicalNodeEnum::Project(project),
    };
    let mut filter = match input {
        LogicalNodeEnum::Filter(filter) => filter,
        other => {
            project.set_input(other);
            return LogicalNodeEnum::Project(project);
        }
    };

    let is_true_filter = filter
        .condition
        .as_literal()
        .is_some_and(|value| matches!(value, graphdb_core::Value::Bool(true)));
    if !is_true_filter {
        project.set_input(LogicalNodeEnum::Filter(filter));
        return LogicalNodeEnum::Project(project);
    }

    match filter.input.take() {
        Some(boxed) if matches!(*boxed, LogicalNodeEnum::ExpandAll(_)) => *boxed,
        taken => {
            filter.input = taken;
            project.set_input(LogicalNodeEnum::Filter(filter));
            LogicalNodeEnum::Project(project)
        }
    }
}

/// Detach the single input of a node without cloning the subtree.
/// The caller receives the owned input and must attach a new one afterwards.
fn take_single_input(input_mut: &mut PlanNodeEnum) -> PlanNodeEnum {
    let placeholder = PlanNodeEnum::Start(StartNode::new());
    std::mem::replace(input_mut, placeholder)
}

/// Rewrites in place through `&mut` so deep pipe chains only hold two
/// pointers per recursion level instead of two owned trees.
fn replace_argument_node(plan: &mut PlanNodeEnum, replacement: &PlanNodeEnum) {
    if matches!(plan, PlanNodeEnum::Argument(_) | PlanNodeEnum::Start(_)) {
        *plan = replacement.clone();
        return;
    }
    // Pipe DELETE wiring consumes the whole node; take it out first so the
    // replacement subtree is moved without borrowing the node being replaced.
    if matches!(
        plan,
        PlanNodeEnum::DeleteVertices(_) | PlanNodeEnum::DeleteEdges(_)
    ) {
        let placeholder = PlanNodeEnum::Start(StartNode::new());
        let owned = std::mem::replace(plan, placeholder);
        let wired = match owned {
            PlanNodeEnum::DeleteVertices(delete_vertices) => {
                let info = delete_vertices.info().clone();
                let node = PipeDeleteVerticesNode::new(next_node_id(), info, replacement.clone());
                PlanNodeEnum::PipeDeleteVertices(node)
            }
            PlanNodeEnum::DeleteEdges(delete_edges) => {
                let info = delete_edges.info().clone();
                let node = PipeDeleteEdgesNode::new(next_node_id(), info, replacement.clone());
                PlanNodeEnum::PipeDeleteEdges(node)
            }
            _ => unreachable!("variant checked above"),
        };
        *plan = wired;
        return;
    }
    match plan {
        PlanNodeEnum::Project(project) => {
            let mut input = take_single_input(project.input_mut());
            replace_argument_node(&mut input, replacement);
            project.set_input(input);
        }
        PlanNodeEnum::Aggregate(aggregate) => {
            // A standalone GROUP BY is planned as Aggregate -> Project -> Scan.
            // When the GROUP BY appears on the right side of a pipe, replace the
            // whole adapter with the left plan so the aggregate consumes the
            // piped rows directly.
            let input = take_single_input(aggregate.input_mut());
            let new_input = match input {
                PlanNodeEnum::Project(mut project) => {
                    if matches!(project.input(), PlanNodeEnum::ScanVertices(_)) {
                        replacement.clone()
                    } else {
                        let mut project_input = take_single_input(project.input_mut());
                        replace_argument_node(&mut project_input, replacement);
                        project.set_input(project_input);
                        PlanNodeEnum::Project(project)
                    }
                }
                mut other => {
                    replace_argument_node(&mut other, replacement);
                    other
                }
            };
            aggregate.set_input(new_input);
        }
        PlanNodeEnum::Filter(filter) => {
            let mut input = take_single_input(filter.input_mut());
            replace_argument_node(&mut input, replacement);
            filter.set_input(input);
        }
        PlanNodeEnum::Sort(sort) => {
            let mut input = take_single_input(sort.input_mut());
            replace_argument_node(&mut input, replacement);
            sort.set_input(input);
        }
        PlanNodeEnum::Limit(limit) => {
            let mut input = take_single_input(limit.input_mut());
            replace_argument_node(&mut input, replacement);
            limit.set_input(input);
        }
        PlanNodeEnum::Dedup(dedup) => {
            let mut input = take_single_input(dedup.input_mut());
            replace_argument_node(&mut input, replacement);
            dedup.set_input(input);
        }
        PlanNodeEnum::Unwind(unwind) => {
            let mut new_col_names = replacement.col_names().to_vec();
            if let Some(alias) = unwind.col_names().last().cloned() {
                new_col_names.push(alias);
            }
            let mut input = take_single_input(unwind.input_mut());
            replace_argument_node(&mut input, replacement);
            unwind.set_input(input);

            unwind.set_col_names(new_col_names);
        }
        PlanNodeEnum::PipeDeleteVertices(pipe_delete_vertices) => {
            let mut input = take_single_input(pipe_delete_vertices.input_mut());
            replace_argument_node(&mut input, replacement);
            pipe_delete_vertices.set_input(input);
        }
        PlanNodeEnum::PipeDeleteEdges(pipe_delete_edges) => {
            let mut input = take_single_input(pipe_delete_edges.input_mut());
            replace_argument_node(&mut input, replacement);
            pipe_delete_edges.set_input(input);
        }
        PlanNodeEnum::ExpandAll(expand) => {
            for inp in expand.inputs_mut() {
                replace_argument_node(inp, replacement);
            }
        }
        PlanNodeEnum::GetVertices(gv) => {
            for inp in gv.inputs_mut() {
                replace_argument_node(inp, replacement);
            }
        }
        _ => {}
    }
}
