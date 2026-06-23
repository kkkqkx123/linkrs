//! Implementation of a plan rewriter
//!
//! Manage all heuristic rewriting rules and apply them to the planning tree in the specified order.
//! Using static distribution (enumeration) in place of dynamic distribution provides better performance.
//!
//! # Performance Advantages
//!
//! No dynamic distribution overhead (no lookup in virtual function tables).
//! No heap allocation (the rules are stored on the stack).
//! Better cache locality
//! Compilers can perform in-line optimizations.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::result::RewriteResult;
use crate::query::optimizer::heuristic::rule_enum::{RewriteRule as RewriteRuleEnum, RuleRegistry};
use crate::query::optimizer::heuristic::visitor::ChildRewriteVisitor;
use crate::query::planning::plan::ExecutionPlan;
use crate::query::planning::plan::PlanNodeEnum;

/// Plan Rewriter
///
/// Manage all heuristic rewriting rules and apply them in order.
/// The rules are executed in the order in which they were added. Each rule may be applied multiple times until no further changes occur.
///
/// Use static distribution of enumeration-based storage rules to avoid the overhead associated with dynamic distribution.
#[derive(Debug)]
pub struct PlanRewriter {
    /// List of registered rules (static distribution)
    rules: Vec<RewriteRuleEnum>,
    /// The maximum number of iterations, to prevent an infinite loop.
    max_iterations: usize,
}

impl PlanRewriter {
    /// Create a new plan rewriter.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            max_iterations: 100,
        }
    }

    /// Created from the rule registry.
    pub fn from_registry(registry: RuleRegistry) -> Self {
        Self {
            rules: registry.into_vec(),
            max_iterations: 100,
        }
    }

    /// Set the maximum number of iterations.
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    /// Add rules
    pub fn add_rule(&mut self, rule: RewriteRuleEnum) {
        self.rules.push(rule);
    }

    /// Add rules in batches
    pub fn add_rules(&mut self, rules: impl IntoIterator<Item = RewriteRuleEnum>) {
        self.rules.extend(rules);
    }

    /// Obtain the number of rules
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Clear rules
    pub fn clear_rules(&mut self) {
        self.rules.clear();
    }

    /// Rewrite the execution plan
    ///
    /// Apply the iteration rules to all registered items until the plan no longer changes, or until the maximum number of iterations has been reached.
    pub fn rewrite(&self, plan: ExecutionPlan) -> RewriteResult<ExecutionPlan> {
        let root = match plan.root {
            Some(ref root) => root.clone(),
            None => return Ok(plan),
        };

        let mut ctx = RewriteContext::new();
        let root_id = ctx.allocate_node_id();
        let new_root = self.rewrite_node(&mut ctx, &root, root_id)?;

        let mut new_plan = plan;
        new_plan.set_root(new_root);
        Ok(new_plan)
    }

    /// Rewrite a single plan node
    pub(crate) fn rewrite_node(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
        node_id: usize,
    ) -> RewriteResult<PlanNodeEnum> {
        // First, rewrite the child nodes recursively.
        let node = self.rewrite_children(ctx, node)?;

        // Registering nodes with the context
        ctx.register_node(node_id, node.clone());

        // Apply the iteration rules until convergence is achieved.
        let mut current_node = node;
        let mut changed = true;
        let mut iterations = 0;

        while changed && iterations < self.max_iterations {
            changed = false;
            iterations += 1;

            for rule in &self.rules {
                // Check whether the rules are matched.
                if rule.matches(&current_node) {
                    // Apply the rules.
                    if let Some(result) = rule.apply(ctx, &current_node)? {
                        if let Some(new_node) = result.first_new_node() {
                            current_node = new_node.clone();
                            changed = true;
                        }
                    }
                }
            }
        }

        Ok(current_node)
    }

    /// Recursive rewriting of child nodes
    ///
    /// Use the Visitor pattern to traverse the planning tree, in order to avoid duplicate code for pattern matching.
    /// The ChildRewriteVisitor class implements the rewriting logic for all types of nodes.
    fn rewrite_children(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<PlanNodeEnum> {
        let mut visitor = ChildRewriteVisitor::new(ctx, self);
        node.accept(&mut visitor)
    }
}

impl Default for PlanRewriter {
    fn default() -> Self {
        Self::from_registry(RuleRegistry::default())
    }
}

/// Create a default plan rewriter.
pub fn create_default_rewriter() -> PlanRewriter {
    PlanRewriter::default()
}

/// Convenient functions for rewriting the execution plan
pub fn rewrite_plan(plan: ExecutionPlan) -> RewriteResult<ExecutionPlan> {
    let rewriter = create_default_rewriter();
    rewriter.rewrite(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_rewriter_default() {
        let rewriter = PlanRewriter::default();
        assert!(rewriter.rule_count() > 0);
    }

    #[test]
    fn test_plan_rewriter_new() {
        let rewriter = PlanRewriter::new();
        assert_eq!(rewriter.rule_count(), 0);
    }

    #[test]
    fn test_plan_rewriter_add_rule() {
        use crate::query::optimizer::heuristic::elimination::EliminateFilterRule;

        let mut rewriter = PlanRewriter::new();
        assert_eq!(rewriter.rule_count(), 0);

        rewriter.add_rule(RewriteRuleEnum::EliminateFilter(EliminateFilterRule));

        assert_eq!(rewriter.rule_count(), 1);
    }
}
