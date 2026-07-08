//! Optimization Strategy Trait Module
//!
//! Defines a unified interface for all optimization strategies, enabling
//! strategy composition and decoupling from implementation details.

use crate::query::optimizer::context::OptimizationContext;
use crate::query::optimizer::error::OptimizeResult;
use crate::query::planning::plan::core::nodes::PlanNodeEnum;

/// Optimization strategy trait
///
/// All optimization strategies must implement this trait, providing a unified
/// interface for plan optimization.
pub trait OptimizationStrategy: Send + Sync {
    /// Apply the optimization strategy to a plan node.
    ///
    /// # Parameters
    /// - `node`: The plan node to optimize
    /// - `ctx`: The optimization context containing shared data
    ///
    /// # Returns
    /// The optimized plan node, or an error if optimization fails
    fn apply(&self, node: PlanNodeEnum, ctx: &OptimizationContext) -> OptimizeResult<PlanNodeEnum>;

    /// Get the name of this strategy.
    fn name(&self) -> &str;

    /// Check if this strategy is enabled.
    fn is_enabled(&self) -> bool {
        true
    }
}

/// Optimization strategy chain
///
/// Applies multiple optimization strategies in sequence, allowing for
/// strategy composition and ordering.
pub struct StrategyChain {
    strategies: Vec<Box<dyn OptimizationStrategy>>,
}

impl StrategyChain {
    /// Create a new strategy chain.
    pub fn new() -> Self {
        Self {
            strategies: Vec::new(),
        }
    }

    /// Add a strategy to the chain.
    pub fn add_strategy(mut self, strategy: Box<dyn OptimizationStrategy>) -> Self {
        self.strategies.push(strategy);
        self
    }

    /// Apply all strategies in the chain to a plan node.
    pub fn apply(
        &self,
        mut node: PlanNodeEnum,
        ctx: &OptimizationContext,
    ) -> OptimizeResult<PlanNodeEnum> {
        for strategy in &self.strategies {
            if !strategy.is_enabled() {
                continue;
            }
            node = strategy.apply(node, ctx)?;
        }
        Ok(node)
    }

    /// Get the number of strategies in the chain.
    pub fn len(&self) -> usize {
        self.strategies.len()
    }

    /// Check if the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.strategies.is_empty()
    }
}

impl Default for StrategyChain {
    fn default() -> Self {
        Self::new()
    }
}

/// No-op strategy for testing and fallback
pub struct NoOpStrategy;

impl OptimizationStrategy for NoOpStrategy {
    fn apply(
        &self,
        node: PlanNodeEnum,
        _ctx: &OptimizationContext,
    ) -> OptimizeResult<PlanNodeEnum> {
        Ok(node)
    }

    fn name(&self) -> &str {
        "NoOp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_op_strategy() {
        let strategy = NoOpStrategy;
        assert_eq!(strategy.name(), "NoOp");
        assert!(strategy.is_enabled());
    }

    #[test]
    fn test_strategy_chain() {
        let chain = StrategyChain::new()
            .add_strategy(Box::new(NoOpStrategy))
            .add_strategy(Box::new(NoOpStrategy));

        assert_eq!(chain.len(), 2);
        assert!(!chain.is_empty());
    }

    #[test]
    fn test_empty_strategy_chain() {
        let chain = StrategyChain::new();
        assert_eq!(chain.len(), 0);
        assert!(chain.is_empty());
    }
}
