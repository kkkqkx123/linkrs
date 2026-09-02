use std::sync::Arc;

use crate::binder::validation::ValidatedStatement;
use crate::binder::BoundStatement;
use crate::metadata::MetadataContext;
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
}
