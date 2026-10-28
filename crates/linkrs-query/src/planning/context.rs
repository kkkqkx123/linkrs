use std::sync::Arc;

use crate::binder::BoundStatement;
use crate::metadata::MetadataContext;
use crate::parser::ast::stmt::Ast;
use crate::planning::join_order::JoinOrderStats;
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
    /// Already parsed statement for this query, when the caller has one.
    /// Planners prefer this over re-parsing query text.
    pub ast: Option<Arc<Ast>>,
    /// Optional join-order statistics snapshot, filled by the compiler from
    /// the same statistics manager the optimizer reviews against.
    /// The match planner reads this instead of an external setter.
    pub join_order_stats: Option<JoinOrderStats>,
}

impl<'a> PlanContext<'a> {
    /// Create a new PlanContext.
    pub fn new(
        bound: &'a BoundStatement,
        qctx: Arc<QueryContext>,
        metadata: Option<&'a MetadataContext>,
    ) -> Self {
        Self {
            bound,
            qctx,
            metadata,
            ast: None,
            join_order_stats: None,
        }
    }

    /// Attach the already parsed statement for this query.
    pub fn with_ast(mut self, ast: Arc<Ast>) -> Self {
        self.ast = Some(ast);
        self
    }

    /// Attach the join-order statistics snapshot for this query.
    pub fn with_join_order_stats(mut self, stats: JoinOrderStats) -> Self {
        self.join_order_stats = Some(stats);
        self
    }

    /// Create a PlanContext with metadata injected later.
    pub fn without_metadata(bound: &'a BoundStatement, qctx: Arc<QueryContext>) -> Self {
        Self {
            bound,
            qctx,
            metadata: None,
            ast: None,
            join_order_stats: None,
        }
    }

    /// Shorthand: return a sub-context containing only bound + qctx + metadata,
    /// for recursive planners (Pipe, SetOperation) that forward to sub-planners.
    pub fn as_inner(&self) -> PlanContext<'_> {
        PlanContext {
            bound: self.bound,
            qctx: self.qctx.clone(),
            metadata: self.metadata,
            ast: self.ast.clone(),
            join_order_stats: self.join_order_stats.clone(),
        }
    }

    /// Replace the bound statement (for recursive planning of sub-statements).
    pub fn with_bound<'b>(&'b self, bound: &'b BoundStatement) -> PlanContext<'b> {
        PlanContext {
            bound,
            qctx: self.qctx.clone(),
            metadata: self.metadata,
            ast: self.ast.clone(),
            join_order_stats: self.join_order_stats.clone(),
        }
    }
}
