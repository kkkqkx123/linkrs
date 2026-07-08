//! Rewriting the definition of a trait
//!
//! This module provides the definition of a trait that encompasses heuristic rules for rewriting code.
//! Heuristic rules do not rely on cost calculations and always generate either better or equivalent plans.
//!
//! This is a version that has been separated from the optimizer layer and focuses on the requirements of the planner layer.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{MatchedResult, RewriteResult, TransformResult};
use crate::query::planning::plan::PlanNodeEnum;

/// Rewrite the rule for the `trait`.
///
/// All heuristic rewriting rules must implement this trait.
/// The rules identify specific structures in the plan tree through pattern matching, and then the transformations are applied.
///
/// # Example
/// ```rust
/// use crate::query::optimizer::heuristic::rule::RewriteRule;
///
/// #[derive(Debug)]
/// struct MyRule;
///
/// impl RewriteRule for MyRule {
///     fn name(&self) -> &str { "MyRule" }
///     
///     fn pattern(&self) -> Pattern {
///         Pattern::new_with_name("Filter")
///     }
///     
///     fn apply(&self, ctx: &mut RewriteContext, node: &PlanNodeEnum) -> RewriteResult<Option<TransformResult>> {
// Implement the rule logic
///         Ok(None)
///     }
/// }
/// ```
pub trait RewriteRule: std::fmt::Debug + Send + Sync {
    /// Rule Name
    fn name(&self) -> &'static str;

    /// Return the pattern of the rule.
    ///
    /// Used for matching the specific structure of the planning tree
    fn pattern(&self) -> Pattern;

    /// Apply the rule for rewriting the text.
    ///
    /// # Parameters
    /// `ctx`: Rewrite the context.
    /// `node`: The current planned node.
    ///
    /// # Return
    /// - `Ok(Some(result))`: 重写成功，返回转换结果
    /// - `Ok(None)`: 不匹配，保持原节点
    /// - `Err(e)`: 重写失败
    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>>;

    /// Matching pattern
    ///
    /// Check whether the current node matches the pattern specified by the rule.
    fn match_pattern(&self, node: &PlanNodeEnum) -> RewriteResult<Option<MatchedResult>> {
        if self.pattern().matches(node) {
            let mut result = MatchedResult::new();
            result.add_node(node.clone());

            // Add dependency nodes
            for dep in node.dependencies() {
                result.add_dependency(dep.clone());
            }

            result.set_root_node(node.clone());
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Check whether the rules are matched.
    ///
    /// Convenient method: It only returns a signal indicating whether a match was found or not; no detailed information is provided.
    fn matches(&self, node: &PlanNodeEnum) -> bool {
        self.pattern().matches(node)
    }
}

/// Basic rewriting rules for traits
///
/// The `trait` is used to identify the basic overriding rules.
pub trait BaseRewriteRule: RewriteRule {}

/// Merging Rules Trait
///
/// Rules for merging two consecutive operations
pub trait MergeRule: RewriteRule {
    /// Check whether it is possible to merge these elements.
    fn can_merge(&self, parent: &PlanNodeEnum, child: &PlanNodeEnum) -> bool;

    /// Create a merged node.
    fn create_merged_node(
        &self,
        ctx: &mut RewriteContext,
        parent: &PlanNodeEnum,
        child: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>>;
}

/// “Push Down Rule” trait
///
/// Rules used to push operations down to the lowest level of the planning tree
pub trait PushDownRule: RewriteRule {
    /// Check whether it is possible to perform a push-down operation.
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool;

    /// Perform the push-down operation.
    fn push_down(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
        target: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>>;
}

/// Remove the “rule” trait.
///
/// Rules for eliminating redundant operations
pub trait EliminationRule: RewriteRule {
    /// Check whether it is possible to eliminate this element/aspect.
    fn can_eliminate(&self, node: &PlanNodeEnum) -> bool;

    /// Perform the elimination operation.
    fn eliminate(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>>;
}

/// Rule Packaging Container
///
/// Used to wrap specific rule types into a unified interface
#[derive(Debug)]
pub struct RuleWrapper<T: RewriteRule> {
    inner: T,
}

impl<T: RewriteRule> RuleWrapper<T> {
    pub fn new(rule: T) -> Self {
        Self { inner: rule }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: RewriteRule> RewriteRule for RuleWrapper<T> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn pattern(&self) -> Pattern {
        self.inner.pattern()
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.inner.apply(ctx, node)
    }
}

/// Rule Adaptation Adapter trait
///
/// It is allowed to convert specific rules into wrappers.
pub trait IntoRuleWrapper: RewriteRule + Sized {
    fn into_wrapper(self) -> RuleWrapper<Self> {
        RuleWrapper::new(self)
    }
}

impl<T: RewriteRule + Sized> IntoRuleWrapper for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    #[derive(Debug)]
    struct TestRule;

    impl RewriteRule for TestRule {
        fn name(&self) -> &'static str {
            "TestRule"
        }

        fn pattern(&self) -> Pattern {
            Pattern::new_with_name("ScanVertices")
        }

        fn apply(
            &self,
            _ctx: &mut RewriteContext,
            _node: &PlanNodeEnum,
        ) -> RewriteResult<Option<TransformResult>> {
            Ok(None)
        }
    }

    #[test]
    fn test_rule_name() {
        let rule = TestRule;
        assert_eq!(rule.name(), "TestRule");
    }

    #[test]
    fn test_rule_matches() {
        let rule = TestRule;
        let node = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "default"));

        assert!(rule.matches(&node));
    }

    #[test]
    fn test_rule_wrapper() {
        let rule = TestRule;
        let wrapper = rule.into_wrapper();

        assert_eq!(wrapper.name(), "TestRule");
    }
}
