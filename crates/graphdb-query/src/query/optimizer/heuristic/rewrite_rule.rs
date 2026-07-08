//! Rewriting the definition of a trait
//!
//! This module provides the definition of a trait that encompasses heuristic rules for rewriting code.
//! Heuristic rules do not rely on cost calculations and always produce either better or equivalent plans.
//!
//! The current implementation uses types that are independent of the planner layer and no longer relies on the optimizer module.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::PlanNodeEnum;

/// Heuristic rewriting rule trait (compatibility layer)
///
/// A simplified interface provided for simple rewriting rules that do not require the full context of the optimizer.
/// These rules always generate better or equivalent plans, without the need for cost calculations.
///
/// For rules that require access to the optimizer’s status or cost information, please use the `RewriteRule` trait.
pub trait HeuristicRule: std::fmt::Debug + Send + Sync {
    /// Rule Name
    fn name(&self) -> &'static str;

    /// Check whether it matches the current planned node.
    fn matches(&self, node: &PlanNodeEnum) -> bool;

    /// Apply the rule for rewriting the text.
    ///
    /// # Parameters
    /// `ctx`: Rewrite the context.
    /// `node`: The current planned node
    ///
    /// # Return
    /// - `Ok(Some(node))`: 重写成功，返回新节点
    /// - `Ok(None)`: 不匹配，保持原节点
    /// - `Err(e)`: 重写失败
    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<PlanNodeEnum>>;
}

/// Create a wrapper that adapts `HeuristicRule` to `RewriteRule`.
#[derive(Debug)]
pub struct HeuristicRuleAdapter<T: HeuristicRule> {
    inner: T,
}

impl<T: HeuristicRule> HeuristicRuleAdapter<T> {
    pub fn new(rule: T) -> Self {
        Self { inner: rule }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: HeuristicRule> RewriteRule for HeuristicRuleAdapter<T> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn pattern(&self) -> Pattern {
        // Heuristic rules use wildcard patterns, and precise matching is performed by the matches method.
        Pattern::new()
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match self.inner.apply(ctx, node)? {
            Some(new_node) => {
                let mut result = TransformResult::new();
                result.add_new_node(new_node);
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }

    fn matches(&self, node: &PlanNodeEnum) -> bool {
        self.inner.matches(node)
    }
}

/// Provide a trait for the adapter constructor of HeuristicRule
pub trait IntoOptRule: HeuristicRule + Sized {
    fn into_opt_rule(self) -> HeuristicRuleAdapter<Self> {
        HeuristicRuleAdapter::new(self)
    }
}

impl<T: HeuristicRule + Sized> IntoOptRule for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    #[derive(Debug)]
    struct TestHeuristicRule;

    impl HeuristicRule for TestHeuristicRule {
        fn name(&self) -> &'static str {
            "TestHeuristicRule"
        }

        fn matches(&self, node: &PlanNodeEnum) -> bool {
            node.is_scan_vertices()
        }

        fn apply(
            &self,
            _ctx: &mut RewriteContext,
            node: &PlanNodeEnum,
        ) -> RewriteResult<Option<PlanNodeEnum>> {
            if self.matches(node) {
                // Return to the original node (the actual rule would involve more complex logic.)
                Ok(Some(node.clone()))
            } else {
                Ok(None)
            }
        }
    }

    #[test]
    fn test_heuristic_rule_adapter() {
        let rule = TestHeuristicRule;
        let adapter = rule.into_opt_rule();

        assert_eq!(adapter.name(), "TestHeuristicRule");

        let node = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "default"));
        assert!(adapter.matches(&node));
    }

    #[test]
    fn test_heuristic_rule_apply() {
        let rule = TestHeuristicRule;
        let mut ctx = RewriteContext::new();
        let node = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "default"));

        let result = rule
            .apply(&mut ctx, &node)
            .expect("Failed to apply rewrite rule");
        assert!(result.is_some());
    }
}
