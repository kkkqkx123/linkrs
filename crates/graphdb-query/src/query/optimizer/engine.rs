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

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::query::optimizer::cost_based::subquery_unnesting::UnnestDecision;
use crate::query::optimizer::heuristic::PlanRewriter;
use crate::query::optimizer::partitioning::{PartitioningConfig, PartitioningPlanner};
use crate::query::optimizer::{
    AggregateStrategySelector, BatchPlanAnalyzer, CostCalculator, CostModelConfig, CteCacheManager,
    SelectivityEstimator, SelectivityFeedbackManager, SortEliminationOptimizer, StatisticsManager,
    SubqueryUnnestingOptimizer,
};

use crate::query::planning::plan::ExecutionPlan;
use crate::query::planning::plan::PlanNodeEnum;

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
    /// Cost model configuration
    cost_config: CostModelConfig,
    /// Heuristic plan rewriter
    heuristic_rewriter: PlanRewriter,
    /// Conservative selector for physical streaming partitions.
    partitioning_planner: PartitioningPlanner,
    /// Enable heuristic optimization phase
    enable_heuristic: bool,
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
            cost_config,
            heuristic_rewriter,
            partitioning_planner: PartitioningPlanner::new(PartitioningConfig::default()),
            enable_heuristic: true,
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

    /// Set the maximum number of heuristic iterations
    pub fn set_max_heuristic_iterations(&mut self, max: usize) {
        self.max_heuristic_iterations = max;
        log::info!(
            "The maximum number of heuristic iterations has been set to {}",
            max
        );
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
            current_plan = self
                .apply_heuristic_with_max_iterations(current_plan, self.max_heuristic_iterations)?;
            log::debug!("Phase 1 completed successfully");
        }

        // Phase 2: Cost-Based Optimization (always active — conservative rules)
        log::debug!("Starting Phase 2: Cost-Based Optimization");
        current_plan = self.apply_cost_based(current_plan)?;
        log::debug!("Phase 2 completed successfully");

        current_plan = self.apply_partitioning_selection(current_plan);

        Ok(current_plan)
    }

    fn apply_partitioning_selection(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        if plan.partition_spec().is_some() {
            return plan;
        }
        let Some(root) = plan.root.as_ref() else {
            return plan;
        };
        let decision = self.partitioning_planner.decide(root, &self.stats_manager);
        if let Some(spec) = decision.partition_spec {
            log::debug!("Selected partition layout: {}", decision.reason);
            plan.set_partition_spec(spec);
        }
        plan
    }

    /// Replace the conservative partitioning configuration. This is intended
    /// for database setup, before the engine is shared by query pipelines.
    pub fn set_partitioning_config(&mut self, config: PartitioningConfig) {
        self.partitioning_planner = PartitioningPlanner::new(config);
    }

    pub fn partitioning_config(&self) -> &PartitioningConfig {
        self.partitioning_planner.config()
    }

    /// Apply heuristic optimization rules with caller-supplied iteration limit.
    fn apply_heuristic_with_max_iterations(
        &self,
        plan: ExecutionPlan,
        max_iterations: usize,
    ) -> OptimizeResult<ExecutionPlan> {
        // Interior mutability via Cell: set_max_iterations does not need &mut self.
        self.heuristic_rewriter.set_max_iterations(max_iterations);
        self.heuristic_rewriter
            .rewrite(plan)
            .map_err(|e| OptimizeError::HeuristicFailed(e.to_string()))
    }

    /// Apply cost-based optimization strategies.
    ///
    /// Currently performs:
    /// - Subquery unnesting: PatternApply → HashInnerJoin when cost-beneficial
    /// - Join order optimization: reorder joins based on estimated costs
    fn apply_cost_based(&self, plan: ExecutionPlan) -> OptimizeResult<ExecutionPlan> {
        let mut plan = plan;

        // Phase 1: Subquery unnesting (PatternApply → HashInnerJoin)
        let root_clone = plan.root.clone();
        if let Some(ref root) = root_clone {
            let rewritten = self.unnest_subqueries(root);
            plan.set_root(rewritten);
        }

        // Phase 2: Join order optimization (extract tables/conditions, reorder)
        if let Some(ref root) = plan.root.clone() {
            let rewritten = crate::query::optimizer::cost_based::join_order_rewriter::walk_and_optimize_joins(
                root,
                &self.stats_manager,
                &self.cost_calculator,
            );
            plan.set_root(rewritten);
        }

        Ok(plan)
    }

    /// Recursively walk the plan tree and rewrite PatternApply → HashInnerJoin
    /// when the subquery unnesting optimizer determines it is beneficial.
    fn unnest_subqueries(&self, node: &PlanNodeEnum) -> PlanNodeEnum {
        use PlanNodeEnum::*;

        // Try PatternApply unnesting at this level first.
        if let PatternApply(apply) = node {
            let analysis = self.batch_plan_analyzer.analyze(node);
            if let UnnestDecision::ShouldUnnest { ref reason, .. } =
                self.subquery_unnesting_optimizer.should_unnest(apply, &analysis)
            {
                log::debug!(
                    "CBO: unnesting PatternApply -> HashInnerJoin ({:?})",
                    reason
                );
                if let Ok(join) = self.subquery_unnesting_optimizer.unnest(apply.clone()) {
                    return self.unnest_subqueries(&join);
                }
            }
        }

        // Recursively rewrite children for the most common node types.
        // Unsupported variants fall through to the catch-all and are returned
        // unchanged (their subtrees are not traversed for unnesting).
        use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
        macro_rules! rewrite_single {
            ($n:expr) => {{
                let mut cloned = $n.clone();
                let new_input = self.unnest_subqueries(cloned.input());
                cloned.set_input(new_input);
                cloned
            }};
        }
        macro_rules! rewrite_binary {
            ($n:expr) => {{
                let mut cloned = $n.clone();
                let new_left = self.unnest_subqueries(cloned.left_input());
                let new_right = self.unnest_subqueries(cloned.right_input());
                cloned.set_left_input(new_left);
                cloned.set_right_input(new_right);
                cloned
            }};
        }

        match node {
            // Single-input operation nodes
            Project(n) => Project(rewrite_single!(n)),
            Filter(n) => Filter(rewrite_single!(n)),
            Sort(n) => Sort(rewrite_single!(n)),
            Limit(n) => Limit(rewrite_single!(n)),
            TopN(n) => TopN(rewrite_single!(n)),
            Sample(n) => Sample(rewrite_single!(n)),
            Dedup(n) => Dedup(rewrite_single!(n)),
            Aggregate(n) => Aggregate(rewrite_single!(n)),
            Window(n) => Window(rewrite_single!(n)),

            // Binary join nodes
            InnerJoin(n) => InnerJoin(rewrite_binary!(n)),
            LeftJoin(n) => LeftJoin(rewrite_binary!(n)),
            RightJoin(n) => RightJoin(rewrite_binary!(n)),
            CrossJoin(n) => CrossJoin(rewrite_binary!(n)),
            FullOuterJoin(n) => FullOuterJoin(rewrite_binary!(n)),
            SemiJoin(n) => SemiJoin(rewrite_binary!(n)),
            HashInnerJoin(n) => HashInnerJoin(rewrite_binary!(n)),
            HashLeftJoin(n) => HashLeftJoin(rewrite_binary!(n)),

            // PatternApply: unnesting was attempted above; if we reach here
            // the decision was to keep it, so rewrite the left child (the
            // main data pipeline). The right child (subquery pattern) is
            // typically a leaf scan and is left unchanged.
            PatternApply(n) => PatternApply(rewrite_single!(n)),

            // Leaf / unsupported nodes: return unchanged.
            _ => node.clone(),
        }
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

        engine.set_enable_heuristic(true);
        assert!(engine.enable_heuristic);
    }

    #[test]
    fn test_optimizer_engine_max_iterations() {
        let mut engine = OptimizerEngine::default();

        engine.set_max_heuristic_iterations(50);
        assert_eq!(engine.max_heuristic_iterations, 50);
    }
}
