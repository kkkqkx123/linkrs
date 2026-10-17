use std::cell::RefCell;
use std::collections::HashMap;

use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::plan_rewriter::NodeRewriter;
use crate::optimizer::heuristic::result::RewriteResult;
use crate::optimizer::heuristic::rule_enum::RewriteRule;
use crate::optimizer::heuristic::visitor::ChildRewriteVisitor;
use crate::planning::plan::PlanNodeEnum;

use super::subquery_host::subtree_contains_expression_subqueries;

/// Recursive node rewriter that applies one optimization batch's rules
/// bottom-up at every node in the plan tree.
///
/// A single batch's rules are applied at a node until the node stops
/// changing (fixed point), then the walker recurses into the children and
/// applies the rules there. This makes every rule effective regardless of
/// the plan root type (e.g. an Aggregate above a Filter/ExpandAll).
pub struct BatchNodeRewriter<'a> {
    rules: &'a [RewriteRule],
    max_iterations: usize,
    applied: RefCell<usize>,
    hits: RefCell<HashMap<String, usize>>,
}

impl<'a> BatchNodeRewriter<'a> {
    /// Build a rewriter for one batch pass.
    pub(super) fn new(rules: &'a [RewriteRule], max_iterations: usize) -> Self {
        Self {
            rules,
            max_iterations,
            applied: RefCell::new(0),
            hits: RefCell::new(HashMap::new()),
        }
    }

    /// Consume the rewriter, returning the applied-rule count and per-rule hit
    /// counts accumulated during the pass.
    pub(super) fn into_counts(self) -> (usize, HashMap<String, usize>) {
        (self.applied.into_inner(), self.hits.into_inner())
    }
}

impl NodeRewriter for BatchNodeRewriter<'_> {
    fn rewrite_node(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
        node_id: usize,
    ) -> RewriteResult<PlanNodeEnum> {
        // Rewrite children first (bottom-up).
        let mut visitor = ChildRewriteVisitor::new(ctx, self);
        let node = node.accept(&mut visitor)?;

        ctx.register_node(node_id, node.clone());
        ctx.set_current_node_id(node_id);

        // Never rewrite subtrees carrying expression-level
        // subqueries — every rule that rebuilds a Filter/Project/Assign
        // host would drop its compiled subquery runners. The nodes still
        // flow through to the arena builder untouched.
        if subtree_contains_expression_subqueries(&node) {
            return Ok(node);
        }

        // Apply the batch rules at this node until fixed point.
        let mut current_node = node;
        let mut changed = true;
        let mut iterations = 0;
        while changed && iterations < self.max_iterations {
            changed = false;
            iterations += 1;
            for rule in self.rules {
                if rule.matches(&current_node) {
                    if let Some(result) = rule.apply(ctx, &current_node)? {
                        if let Some(new_node) = result.first_new_node() {
                            current_node = new_node.clone();
                            *self.applied.borrow_mut() += 1;
                            *self
                                .hits
                                .borrow_mut()
                                .entry(rule.name().to_string())
                                .or_insert(0) += 1;
                            changed = true;
                        }
                    }
                }
            }
        }

        Ok(current_node)
    }
}
