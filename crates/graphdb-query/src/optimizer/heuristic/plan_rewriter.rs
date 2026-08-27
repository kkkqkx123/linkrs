//! Node rewriter trait shared by the batch optimizer's node walker and the
//! legacy single-loop plan rewriter.
//!
//! The legacy [`PlanRewriter`] was removed as part of converging the heuristic
//! engine onto the single [`BatchOptimizer`](crate::optimizer::heuristic::batch::BatchOptimizer);
//! the `NodeRewriter` trait remains because both the batch optimizer's node
//! walker and the [`ChildRewriteVisitor`](super::visitor::ChildRewriteVisitor)
//! drive the same child-walking traversal.

use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::result::RewriteResult;
use crate::planning::plan::PlanNodeEnum;

/// Trait for a node rewriter used by [`ChildRewriteVisitor`](super::visitor::ChildRewriteVisitor).
///
/// The batch optimizer's node walker implements this trait; both the walker
/// and the shared child-walking visitor keep the tree traversal in one place.
pub trait NodeRewriter {
    /// Rewrite a single node: first rewrite its children, then apply the
    /// registered rules to the node until the plan stops changing.
    fn rewrite_node(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
        node_id: usize,
    ) -> RewriteResult<PlanNodeEnum>;
}
