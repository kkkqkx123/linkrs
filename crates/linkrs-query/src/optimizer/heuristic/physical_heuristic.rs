//! Physical heuristic optimizer operating on `PlanNodeEnum`.
//!
//! Independent wrapper around [`BatchOptimizer`] for the post-mapping
//! physical phase. Logical rewrites run on `LogicalNodeEnum` via
//! [`LogicalBatchOptimizer`]; this struct owns the physical rule batches
//! so the two phases no longer share one optimizer instance.

use crate::optimizer::heuristic::batch::{BatchOptimizer, OptimizationResult};
use crate::optimizer::heuristic::result::RewriteResult;
use crate::optimizer::heuristic::rule_enum::RuleRegistry;
use crate::planning::plan::PlanNodeEnum;

/// Heuristic optimizer for the physical plan tree.
pub struct PhysicalHeuristicOptimizer {
    batch: BatchOptimizer,
}

impl std::fmt::Debug for PhysicalHeuristicOptimizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhysicalHeuristicOptimizer")
            .field("batch", &self.batch)
            .finish()
    }
}

impl Default for PhysicalHeuristicOptimizer {
    fn default() -> Self {
        Self::from_registry(RuleRegistry::default())
    }
}

impl PhysicalHeuristicOptimizer {
    /// Wrap an existing batch optimizer.
    pub fn new(batch: BatchOptimizer) -> Self {
        Self { batch }
    }

    /// Build from a rule registry using default batch assignment.
    pub fn from_registry(registry: RuleRegistry) -> Self {
        Self {
            batch: BatchOptimizer::from_registry(registry),
        }
    }

    /// Optimize a physical plan tree.
    pub fn optimize(&self, plan: PlanNodeEnum) -> RewriteResult<OptimizationResult> {
        self.batch.optimize(plan)
    }

    /// Override the iteration budget of the wrapped batches.
    pub fn set_max_iterations(&self, max: usize) {
        self.batch.set_max_iterations(max);
    }

    /// Access the wrapped batch optimizer.
    pub fn batch(&self) -> &BatchOptimizer {
        &self.batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;

    #[test]
    fn physical_heuristic_optimizes_without_rules_firing() {
        let optimizer = PhysicalHeuristicOptimizer::default();
        let plan = PlanNodeEnum::Start(StartNode::new());
        let result = optimizer.optimize(plan).expect("optimize");
        assert!(matches!(result.optimized_plan, PlanNodeEnum::Start(_)));
    }
}
