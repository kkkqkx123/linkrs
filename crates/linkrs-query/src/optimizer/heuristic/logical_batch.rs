//! Logical batch optimizer operating on `LogicalNodeEnum`.
//!
//! Fixed-point driver for [`LogicalRule`]s. Rules are applied bottom-up:
//! children are optimized before the rule runs at the current node. The
//! optimizer iterates over the rule set until no rule changes the tree or
//! the iteration budget is exhausted.

use crate::optimizer::error::OptimizeResult;
use crate::optimizer::heuristic::logical_rule::{LogicalRule, LogicalRuleContext};
use crate::planning::plan::logical::LogicalNodeEnum;

/// Outcome of a logical heuristic optimization run.
#[derive(Debug, Clone, Default)]
pub struct LogicalOptimizationResult {
    /// Fixed-point iterations performed.
    pub iterations: usize,
    /// Number of rule applications that changed the tree.
    pub rules_applied: usize,
    /// Whether the tree changed at all.
    pub changed: bool,
}

/// Batch optimizer for logical rewrite rules.
pub struct LogicalBatchOptimizer {
    rules: Vec<Box<dyn LogicalRule>>,
    max_iterations: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for LogicalBatchOptimizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogicalBatchOptimizer")
            .field(
                "rules",
                &self.rules.iter().map(|r| r.name()).collect::<Vec<_>>(),
            )
            .field(
                "max_iterations",
                &self
                    .max_iterations
                    .load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish()
    }
}

impl Default for LogicalBatchOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl LogicalBatchOptimizer {
    /// Create an optimizer with no rules.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            max_iterations: std::sync::atomic::AtomicUsize::new(100),
        }
    }

    /// Create an optimizer with the given rules.
    pub fn with_rules(rules: Vec<Box<dyn LogicalRule>>) -> Self {
        Self {
            rules,
            max_iterations: std::sync::atomic::AtomicUsize::new(100),
        }
    }

    /// Override the fixed-point iteration budget.
    pub fn with_max_iterations(self, max: usize) -> Self {
        self.set_max_iterations(max);
        self
    }

    /// Override the fixed-point iteration budget via shared reference.
    pub fn set_max_iterations(&self, max: usize) {
        self.max_iterations
            .store(max, std::sync::atomic::Ordering::Relaxed);
    }

    /// Register an additional rule.
    pub fn add_rule(&mut self, rule: Box<dyn LogicalRule>) {
        self.rules.push(rule);
    }

    /// Number of registered rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether any rule is registered.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Optimize a logical tree in place.
    pub fn optimize(
        &self,
        plan: &mut LogicalNodeEnum,
    ) -> OptimizeResult<LogicalOptimizationResult> {
        let mut result = LogicalOptimizationResult::default();
        if self.rules.is_empty() {
            return Ok(result);
        }
        let mut ctx = LogicalRuleContext::new();
        let max_iterations = self
            .max_iterations
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut changed = true;
        let mut iterations = 0;
        while changed && iterations < max_iterations {
            changed = false;
            for rule in &self.rules {
                let rule_changed = self.apply_rule_bottom_up(rule.as_ref(), plan, &mut ctx)?;
                if rule_changed {
                    changed = true;
                    result.rules_applied += 1;
                }
            }
            iterations += 1;
        }
        result.iterations = iterations;
        result.changed = ctx.changed;
        Ok(result)
    }

    fn apply_rule_bottom_up(
        &self,
        rule: &dyn LogicalRule,
        node: &mut LogicalNodeEnum,
        ctx: &mut LogicalRuleContext,
    ) -> OptimizeResult<bool> {
        let mut changed = false;
        for child in node.children_mut() {
            changed |= self.apply_rule_bottom_up(rule, child, ctx)?;
        }
        if rule.apply(node, ctx)? {
            ctx.mark_changed();
            changed = true;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;

    #[derive(Debug)]
    struct CountRule {
        hits: std::sync::Mutex<usize>,
    }

    impl LogicalRule for CountRule {
        fn name(&self) -> &str {
            "CountRule"
        }

        fn apply(
            &self,
            _node: &mut LogicalNodeEnum,
            _ctx: &mut LogicalRuleContext,
        ) -> OptimizeResult<bool> {
            *self.hits.lock().expect("mutex") += 1;
            Ok(false)
        }
    }

    #[test]
    fn empty_optimizer_is_noop() {
        let optimizer = LogicalBatchOptimizer::new();
        let mut plan = LogicalNodeEnum::Start(LogicalStartNode::new());
        let result = optimizer.optimize(&mut plan).expect("optimize");
        assert_eq!(result.iterations, 0);
        assert!(!result.changed);
    }

    #[test]
    fn bottom_up_visits_every_node() {
        let rule = CountRule {
            hits: std::sync::Mutex::new(0),
        };
        let optimizer = LogicalBatchOptimizer::with_rules(vec![Box::new(rule)]);
        let mut plan = LogicalNodeEnum::Start(LogicalStartNode::new());
        let result = optimizer.optimize(&mut plan).expect("optimize");
        assert_eq!(result.iterations, 1);
        assert_eq!(result.rules_applied, 0);
    }
}
