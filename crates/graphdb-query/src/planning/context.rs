use std::sync::Arc;

use crate::binder::validation::ValidatedStatement;
use crate::binder::BoundStatement;
use crate::metadata::MetadataContext;
use crate::parser::ast::stmt::{Ast, Stmt};
use crate::QueryContext;

/// Unified context for the plan_bound phase.
///
/// Bundles all inputs needed by statement planners during bound-statement
/// planning. Callers construct a PlanContext once; planners destructure
/// only the fields they need.
#[derive(Debug, Clone)]
pub struct PlanContext<'a> {
    /// The bound (post-binder) statement to plan.
    pub bound: &'a BoundStatement,
    /// Query-level context (space, session, parameters).
    pub qctx: Arc<QueryContext>,
    /// Pre-resolved metadata for referenced tags/edges/indexes.
    /// Only populated when schema_manager is available.
    pub metadata: Option<&'a MetadataContext>,
    /// AST-derived validated statement (expression analysis context).
    /// Needed by planners that still use AST-level info (e.g., MATCH YIELD columns).
    pub validated: &'a ValidatedStatement,
}

impl<'a> PlanContext<'a> {
    /// Create a new PlanContext.
    pub fn new(
        bound: &'a BoundStatement,
        qctx: Arc<QueryContext>,
        metadata: Option<&'a MetadataContext>,
        validated: &'a ValidatedStatement,
    ) -> Self {
        Self {
            bound,
            qctx,
            metadata,
            validated,
        }
    }

    /// Create a PlanContext with metadata injected later.
    pub fn without_metadata(
        bound: &'a BoundStatement,
        qctx: Arc<QueryContext>,
        validated: &'a ValidatedStatement,
    ) -> Self {
        Self {
            bound,
            qctx,
            metadata: None,
            validated,
        }
    }

    /// Shorthand: return a sub-context containing only bound + qctx + validated,
    /// for recursive planners (Pipe, SetOperation) that forward to sub-planners.
    pub fn as_inner(&self) -> PlanContext<'_> {
        PlanContext {
            bound: self.bound,
            qctx: self.qctx.clone(),
            metadata: self.metadata,
            validated: self.validated,
        }
    }

    /// Replace the bound statement (for recursive planning of sub-statements).
    pub fn with_bound<'b>(&'b self, bound: &'b BoundStatement) -> PlanContext<'b> {
        PlanContext {
            bound,
            qctx: self.qctx.clone(),
            metadata: self.metadata,
            validated: self.validated,
        }
    }

    /// Derive the `ValidatedStatement` aligned with a child bound statement.
    ///
    /// Composite planners (Pipe, SetOperation, Explain) forward a shared
    /// `validated` that describes the whole query, while each stage planner
    /// expects the AST fragment matching its own bound sub-statement (e.g.
    /// `MatchStatementPlanner` requires `Stmt::Match`). The binder mirrors
    /// the AST structure exactly, so the fragment is located by walking the
    /// parent AST and parent bound statement in parallel.
    ///
    /// Returns `None` when the structures do not mirror each other; callers
    /// then keep the parent `validated` (previous behavior).
    pub fn derive_validated(&self, child: &BoundStatement) -> Option<ValidatedStatement> {
        let stmt = find_aligned_stmt(self.validated.stmt(), self.bound, child)?;
        Some(ValidatedStatement::new(
            Arc::new(Ast::new(
                stmt.clone(),
                self.validated.ast.expr_context().clone(),
            )),
            self.validated.validation_info.clone(),
        ))
    }
}

/// Locate the AST sub-statement aligned with `child` by walking `stmt` and
/// `bound` in parallel. Identity is by reference: `child` must be borrowed
/// from inside `bound`.
fn find_aligned_stmt<'s>(
    stmt: &'s Stmt,
    bound: &BoundStatement,
    child: &BoundStatement,
) -> Option<&'s Stmt> {
    if std::ptr::eq(bound, child) {
        return Some(stmt);
    }
    match (stmt, bound) {
        (Stmt::Pipe(pipe_stmt), BoundStatement::Pipe(bound_pipe)) => {
            let stmts = [&pipe_stmt.left, &pipe_stmt.right];
            if bound_pipe.statements.len() != stmts.len() {
                return None;
            }
            for (sub_stmt, sub_bound) in stmts.into_iter().zip(bound_pipe.statements.iter()) {
                if let Some(found) = find_aligned_stmt(sub_stmt, sub_bound, child) {
                    return Some(found);
                }
            }
            None
        }
        (Stmt::SetOperation(set_op), BoundStatement::SetOperation(bound_set_op)) => {
            find_aligned_stmt(&set_op.left, &bound_set_op.left, child)
                .or_else(|| find_aligned_stmt(&set_op.right, &bound_set_op.right, child))
        }
        (Stmt::Explain(explain), BoundStatement::Explain(bound_explain)) => {
            find_aligned_stmt(&explain.statement, &bound_explain.statement, child)
        }
        (Stmt::Profile(profile), BoundStatement::Profile(bound_profile)) => {
            find_aligned_stmt(&profile.statement, &bound_profile.statement, child)
        }
        _ => None,
    }
}
