//! Optimizer Engine Module
//!
//! This module provides a query optimization engine, which is responsible for coordinating and managing all components related to query optimization.
//!
//! ## Design Specifications
//!
//! `OptimizerEngine` is the core component of the query optimization layer and is shared and used wherever it is needed through dependency injection.
//! It integrates functions such as statistical information management, cost calculation, and selective estimation, providing a unified optimization service for the query pipeline.
//!
//! ## Explanation of Shared Instances
//!
//! The `OptimizerEngine` is designed to be a component that can be shared across multiple queries for the following reasons:
//!
//! 1. **Sharing of statistical information**: All queries share the same set of statistical information, ensuring consistency in cost estimates.
//! 2. **Resource Efficiency**: Avoid the repeated creation of optimizer components in each query pipeline.
//! 3. **Configuration Consistency**: A unified cost model configuration is applied to all queries.
//!
//! ## How to use it
//!
//! ```rust
// Created during the initialization of the database instance
//! let optimizer_engine = Arc::new(OptimizerEngine::new(CostModelConfig::default()));
//!
// Used in the query pipeline through dependency injection
//! let pipeline = QueryPipelineManager::with_optimizer(storage, stats_manager, optimizer_engine);
//! ```
//!
//! ## Thread Safety
//!
//! `OptimizerEngine` utilizes `Arc` as well as thread-safe data structures, which allow for safe sharing in a multi-threaded environment.
//!
//! ## Attention
//!
//! This is not a global singleton, but an instance that is shared between components through `Arc`. Each database instance can have its own optimizer engine configuration.

use std::sync::Arc;

use crate::query::optimizer::heuristic::PlanRewriter;
use crate::query::optimizer::{
    AggregateStrategySelector, BatchPlanAnalyzer, CostCalculator, CostModelConfig, CteCacheManager,
    MaterializationOptimizer, SelectivityEstimator, SelectivityFeedbackManager,
    SortEliminationOptimizer, StatisticsManager, SubqueryUnnestingOptimizer,
};
use crate::query::planning::plan::ExecutionPlan;
use crate::query::validator::context::ExpressionAnalysisContext;

/// Optimizer engine
///
/// A globally unique instance of the optimizer engine, responsible for coordinating and managing all components related to query optimization.
/// It has the same lifecycle as the database instance and provides unified optimization services for all queries.
#[derive(Debug)]
pub struct OptimizerEngine {
    /// Expression context, used for sharing expression information across different stages
    expression_context: Arc<ExpressionAnalysisContext>,
    /// Statistics Information Manager
    stats_manager: Arc<StatisticsManager>,
    /// Selective Feedback Manager
    selectivity_feedback_manager: Arc<SelectivityFeedbackManager>,
    /// CTE Cache Manager
    cte_cache_manager: Arc<CteCacheManager>,
    /// Cost Calculator
    cost_calculator: Arc<CostCalculator>,
    /// Selective Estimator
    selectivity_estimator: Arc<SelectivityEstimator>,
    /// Sorting Elimination Optimizer
    sort_elimination_optimizer: Arc<SortEliminationOptimizer>,
    /// Aggregation Policy Selector
    aggregate_strategy_selector: AggregateStrategySelector,
    /// Batch plan analyzer (unified analysis)
    batch_plan_analyzer: BatchPlanAnalyzer,
    /// Subquery de-correlating optimizer
    subquery_unnesting_optimizer: SubqueryUnnestingOptimizer,
    /// CTE (Common Table Expression) Materialization Optimizer
    materialization_optimizer: MaterializationOptimizer,
    /// Cost model configuration
    cost_config: CostModelConfig,
    /// Heuristic plan rewriter
    heuristic_rewriter: PlanRewriter,
    /// Enable heuristic optimization phase
    enable_heuristic: bool,
    /// Enable cost-based optimization phase
    enable_cost_based: bool,
    /// Maximum iterations for heuristic rules
    max_heuristic_iterations: usize,
}

impl OptimizerEngine {
    /// Create a new optimizer engine.
    ///
    /// # Parameters
    /// `cost_config`: Configuration of the cost model
    pub fn new(cost_config: CostModelConfig) -> Self {
        Self::with_expression_context(Arc::new(ExpressionAnalysisContext::new()), cost_config)
    }

    /// Create an optimizer engine using the shared ExpressionContext.
    ///
    /// # Parameters
    /// `expression_context`: A shared context for expressions (shared across different stages).
    /// - `cost_config`: Cost model configuration
    pub fn with_expression_context(
        expression_context: Arc<ExpressionAnalysisContext>,
        cost_config: CostModelConfig,
    ) -> Self {
        // Create a statistical information manager
        let stats_manager = Arc::new(StatisticsManager::new());

        // Create a selective feedback manager
        let selectivity_feedback_manager = Arc::new(SelectivityFeedbackManager::new());

        // Create a CTE (Common Table Expression) for cache manager management.
        let cte_cache_manager = Arc::new(CteCacheManager::new());

        Self::with_components(
            expression_context,
            stats_manager,
            selectivity_feedback_manager,
            cte_cache_manager,
            cost_config,
        )
    }

    /// Create an optimizer engine with all components (used by builder).
    ///
    /// This internal constructor allows to builder pattern to inject custom components
    /// while maintaining backward compatibility with existing constructors.
    pub(crate) fn with_components(
        expression_context: Arc<ExpressionAnalysisContext>,
        stats_manager: Arc<StatisticsManager>,
        selectivity_feedback_manager: Arc<SelectivityFeedbackManager>,
        cte_cache_manager: Arc<CteCacheManager>,
        cost_config: CostModelConfig,
    ) -> Self {
        // Create a cost calculator and a selective estimator.
        let cost_calculator = Arc::new(CostCalculator::with_config(
            stats_manager.clone(),
            cost_config,
        ));
        let selectivity_estimator = Arc::new(SelectivityEstimator::new(stats_manager.clone()));

        // Create a sorting elimination optimizer
        let sort_elimination_optimizer =
            Arc::new(SortEliminationOptimizer::new(cost_calculator.clone()));

        // Create batch plan analyzer (unified analysis)
        let batch_plan_analyzer = BatchPlanAnalyzer::new();

        // Create an aggregate policy selector
        let aggregate_strategy_selector = AggregateStrategySelector::new(cost_calculator.clone());

        // Create a subquery to de-associate the optimizer.
        let subquery_unnesting_optimizer = SubqueryUnnestingOptimizer::new(&stats_manager);

        // Creating a CTE (Common Table Expression) materialization optimizer
        let materialization_optimizer = MaterializationOptimizer::with_thresholds(
            &stats_manager,
            &cost_config.strategy_thresholds,
        );

        // Create a heuristic plan rewriter
        let heuristic_rewriter = PlanRewriter::default();

        Self {
            expression_context,
            stats_manager,
            selectivity_feedback_manager,
            cte_cache_manager,
            cost_calculator,
            selectivity_estimator,
            sort_elimination_optimizer,
            aggregate_strategy_selector,
            batch_plan_analyzer,
            subquery_unnesting_optimizer,
            materialization_optimizer,
            cost_config,
            heuristic_rewriter,
            enable_heuristic: true,
            enable_cost_based: true,
            max_heuristic_iterations: 100,
        }
    }

    /// Create an optimized configuration using an SSD.
    pub fn for_ssd() -> Self {
        Self::new(CostModelConfig::for_ssd())
    }

    /// Create an optimized configuration using a memory-based database.
    pub fn for_in_memory() -> Self {
        Self::new(CostModelConfig::for_in_memory())
    }

    /// Obtaining the Cost Model Configuration
    pub fn cost_config(&self) -> &CostModelConfig {
        &self.cost_config
    }

    /// Obtain the Cost Calculator
    pub fn cost_calculator(&self) -> &Arc<CostCalculator> {
        &self.cost_calculator
    }

    /// Statistics Information Manager
    pub fn stats_manager(&self) -> &Arc<StatisticsManager> {
        &self.stats_manager
    }

    /// Obtaining a selective estimator
    pub fn selectivity_estimator(&self) -> &Arc<SelectivityEstimator> {
        &self.selectivity_estimator
    }

    /// Obtaining the sorting elimination optimizer
    pub fn sort_elimination_optimizer(&self) -> &SortEliminationOptimizer {
        &self.sort_elimination_optimizer
    }

    /// Obtain the context of the expression.
    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expression_context
    }

    /// Obtain batch plan analyzer
    pub fn batch_plan_analyzer(&self) -> &BatchPlanAnalyzer {
        &self.batch_plan_analyzer
    }

    /// Obtain the Aggregation Policy Selector
    pub fn aggregate_strategy_selector(&self) -> &AggregateStrategySelector {
        &self.aggregate_strategy_selector
    }

    /// Obtaining the subquery to de-associate the optimizer
    pub fn subquery_unnesting_optimizer(&self) -> &SubqueryUnnestingOptimizer {
        &self.subquery_unnesting_optimizer
    }

    /// Obtaining the CTE (Common Table Expression) materialization optimizer
    pub fn materialization_optimizer(&self) -> &MaterializationOptimizer {
        &self.materialization_optimizer
    }

    /// Obtaining the Selective Feedback Manager
    pub fn selectivity_feedback_manager(&self) -> &SelectivityFeedbackManager {
        &self.selectivity_feedback_manager
    }

    /// Obtaining the CTE Cache Manager
    pub fn cte_cache_manager(&self) -> &CteCacheManager {
        &self.cte_cache_manager
    }

    /// Set the stats manager on the CTE cache manager
    pub fn set_cte_cache_stats_manager(
        &self,
        stats_manager: Arc<crate::core::stats::StatsManager>,
    ) {
        self.cte_cache_manager.set_stats_manager(stats_manager);
    }

    /// Update the Cost Model Configuration
    ///
    /// Updating the configuration will recreate the cost calculator, but it will not affect the existing decision cache.
    pub fn set_cost_config(&mut self, config: CostModelConfig) {
        self.cost_config = config;
        self.cost_calculator = Arc::new(CostCalculator::with_config(
            self.stats_manager.clone(),
            self.cost_config,
        ));
        // Re-create the sorting elimination optimizer, using a new cost calculator.
        self.sort_elimination_optimizer =
            Arc::new(SortEliminationOptimizer::new(self.cost_calculator.clone()));
        // Re-create batch plan analyzer
        self.batch_plan_analyzer = BatchPlanAnalyzer::new();
        // Recreate the Aggregation Policy Selector
        self.aggregate_strategy_selector =
            AggregateStrategySelector::new(self.cost_calculator.clone());
        // Re-create the subquery to de-associate the optimizer.
        self.subquery_unnesting_optimizer = SubqueryUnnestingOptimizer::new(&self.stats_manager);
        // Re-create the CTE (Common Table Expression) materialization optimizer
        self.materialization_optimizer = MaterializationOptimizer::with_thresholds(
            &self.stats_manager,
            &self.cost_config.strategy_thresholds,
        );
        log::info!(
            "Optimizer cost model configuration has been updated: {:?}",
            self.cost_config
        );
    }

    /// Set whether to enable heuristic optimization
    pub fn set_enable_heuristic(&mut self, enable: bool) {
        self.enable_heuristic = enable;
        log::info!(
            "Heuristic optimization has {}",
            if enable {
                "(computing) enable (a feature)"
            } else {
                "prohibit the use of sth."
            }
        );
    }

    /// Set whether to enable cost-based optimization
    pub fn set_enable_cost_based(&mut self, enable: bool) {
        self.enable_cost_based = enable;
        log::info!(
            "Cost-based optimization has {}",
            if enable {
                "(computing) enable (a feature)"
            } else {
                "prohibit the use of sth."
            }
        );
    }

    /// Set the maximum number of heuristic iterations
    pub fn set_max_heuristic_iterations(&mut self, max: usize) {
        self.max_heuristic_iterations = max;
        log::info!(
            "The maximum number of heuristic iterations has been set to {}",
            max
        );
    }

    /// Check if full optimization is enabled (both heuristic and cost-based)
    pub fn is_full_optimization(&self) -> bool {
        self.enable_heuristic && self.enable_cost_based
    }

    /// Optimize an execution plan through all enabled phases
    ///
    /// This is the main entry point for query optimization, coordinating both
    /// heuristic and cost-based optimization phases.
    ///
    /// # Parameters
    /// `plan`: The execution plan to optimize
    ///
    /// # Returns
    /// The optimized execution plan
    pub fn optimize(&self, plan: ExecutionPlan) -> OptimizeResult<ExecutionPlan> {
        let mut current_plan = plan;

        // Phase 1: Heuristic Optimization (Always Executed)
        if self.enable_heuristic {
            log::debug!("Starting Phase 1: Heuristic Optimization");
            current_plan = self.apply_heuristic(current_plan)?;
            log::debug!("Phase 1 completed successfully");
        }

        // Phase 2: Cost-Based Optimization (Optional)
        if self.enable_cost_based {
            log::debug!("Starting Phase 2: Cost-Based Optimization");
            current_plan = self.apply_cost_based(current_plan)?;
            log::debug!("Phase 2 completed successfully");
        }

        Ok(current_plan)
    }

    /// Apply heuristic optimization rules
    fn apply_heuristic(&self, plan: ExecutionPlan) -> OptimizeResult<ExecutionPlan> {
        self.heuristic_rewriter
            .rewrite(plan)
            .map_err(|e| OptimizeError::HeuristicFailed(e.to_string()))
    }

    /// Apply cost-based optimization strategies
    fn apply_cost_based(&self, plan: ExecutionPlan) -> OptimizeResult<ExecutionPlan> {
        use crate::query::optimizer::context::OptimizationContext;
        use crate::query::optimizer::cost_based::trait_def::StrategyChain;
        use crate::query::optimizer::cost_based::MaterializationOptimizer;
        use crate::query::optimizer::cost_based::TraversalDirectionOptimizer;

        // Create optimization context
        let mut ctx = OptimizationContext::from(self);

        // Perform batch plan analysis if we have a root
        let mut current_plan = plan;
        if let Some(ref root) = current_plan.root {
            let batch_analyzer = self.batch_plan_analyzer();
            let batch_analysis = batch_analyzer.analyze(root);
            ctx.set_batch_plan_analysis(batch_analysis);

            // Create optimizers
            let materialization_optimizer =
                MaterializationOptimizer::new(self.stats_manager.as_ref());
            let traversal_direction_optimizer =
                TraversalDirectionOptimizer::new(self.cost_calculator.clone());

            // Create strategy chain with all registered cost-based strategies
            // Order matters: materialization first, then traversal direction
            let chain = StrategyChain::new()
                .add_strategy(Box::new(materialization_optimizer))
                .add_strategy(Box::new(traversal_direction_optimizer));

            // Apply strategies to the plan root
            let optimized_root = chain
                .apply(root.clone(), &ctx)
                .map_err(|e| OptimizeError::CostBasedFailed(e.to_string()))?;

            current_plan.set_root(optimized_root);
        }

        Ok(current_plan)
    }

    /// Get the heuristic rewriter
    pub fn heuristic_rewriter(&self) -> &PlanRewriter {
        &self.heuristic_rewriter
    }
}

use crate::query::optimizer::error::{OptimizeError, OptimizeResult};

impl Default for OptimizerEngine {
    fn default() -> Self {
        Self::new(CostModelConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_engine_creation() {
        let _engine = OptimizerEngine::default();
    }

    #[test]
    fn test_optimizer_engine_with_config() {
        let config = CostModelConfig::for_ssd();
        let _engine = OptimizerEngine::new(config);
    }

    #[test]
    fn test_optimizer_engine_configuration() {
        let mut engine = OptimizerEngine::default();

        // Test enable/disable heuristic
        engine.set_enable_heuristic(false);
        assert!(!engine.enable_heuristic);

        // Test enable/disable cost-based
        engine.set_enable_cost_based(false);
        assert!(!engine.enable_cost_based);

        // Test full optimization check
        assert!(!engine.is_full_optimization());

        engine.set_enable_heuristic(true);
        engine.set_enable_cost_based(true);
        assert!(engine.is_full_optimization());
    }

    #[test]
    fn test_optimizer_engine_max_iterations() {
        let mut engine = OptimizerEngine::default();

        engine.set_max_heuristic_iterations(50);
        assert_eq!(engine.max_heuristic_iterations, 50);
    }
}
