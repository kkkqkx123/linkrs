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
//! use std::sync::Arc;
//! use graphdb_query::query::optimizer::cost::CostModelConfig;
//! use graphdb_query::query::optimizer::engine::OptimizerEngine;
//!
//! // Created during the initialization of the database instance
//! let optimizer_engine = Arc::new(OptimizerEngine::new(CostModelConfig::default()));
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
use std::sync::Mutex;

use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::query::optimizer::cost_based::subquery_unnesting::UnnestDecision;
use crate::query::optimizer::cost_based::{
    AggregateContext, AggregateStrategySelector, IndexSelector, SortEliminationOptimizer,
};
use crate::query::optimizer::heuristic::batch::{
    BatchOptimizer, BatchStatistics, OptimizationBatch,
};
use crate::query::optimizer::heuristic::rule_enum::RuleRegistry;
use crate::query::optimizer::partitioning::{
    PartitioningConfig, PartitioningLayoutInfo, PartitioningPlanner,
};
use crate::query::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
use crate::query::optimizer::stats::feedback::decision::DecisionFeedbackStore;
use crate::query::optimizer::stats::feedback::history::QueryFeedbackHistory;
use crate::query::optimizer::stats::feedback::selectivity::SelectivityFeedbackManager;
use crate::query::optimizer::stats::feedback::trigger::AutoFeedbackTrigger;
use crate::query::optimizer::stats::StatsView;
use crate::query::optimizer::{
    BatchPlanAnalyzer, CostCalculator, CostModelConfig, CteCacheManager, SelectivityEstimator,
    StatisticsManager, SubqueryUnnestingOptimizer,
};

use crate::query::planning::plan::logical_plan::LogicalPlan;
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
    /// CTE Cache Manager
    cte_cache_manager: Arc<CteCacheManager>,
    /// Cost Calculator
    cost_calculator: Arc<CostCalculator>,
    /// Selective Estimator
    selectivity_estimator: Arc<SelectivityEstimator>,
    /// Batch plan analyzer (unified analysis)
    batch_plan_analyzer: BatchPlanAnalyzer,
    /// Subquery de-correlating optimizer
    subquery_unnesting_optimizer: SubqueryUnnestingOptimizer,
    /// Cost model configuration
    cost_config: CostModelConfig,
    /// Heuristic batch optimizer (production heuristic main chain)
    heuristic_batch: BatchOptimizer,
    /// Last batch optimization statistics (exposed for EXPLAIN diagnostics)
    last_batch_statistics: Mutex<Vec<(OptimizationBatch, BatchStatistics)>>,
    /// Conservative selector for physical streaming partitions.
    partitioning_planner: PartitioningPlanner,
    /// Enable heuristic optimization phase
    enable_heuristic: bool,
    /// Maximum iterations for heuristic rules
    max_heuristic_iterations: usize,
    /// Shared query feedback history (stats feedback loop).
    ///
    /// The pipeline injects this history into every execution instance so
    /// that estimated-vs-actual operator feedback is recorded after each
    /// query; selectivity auto-correction consumes it.
    feedback_history: Arc<QueryFeedbackHistory>,
    /// Shared selectivity correction manager (stats feedback loop).
    ///
    /// `maybe_apply_feedback` folds the estimated-vs-actual ratios recorded
    /// in `feedback_history` into per-predicate EWMA corrections; the
    /// `SelectivityEstimator` consults these corrections before falling back
    /// to histogram / heuristic estimates.
    selectivity_feedback: Arc<SelectivityFeedbackManager>,
    /// Shared cardinality correction manager (stats feedback loop).
    ///
    /// `maybe_apply_feedback` folds the estimated-vs-actual ratios of every
    /// shape-keyed operator (scans, traversals, joins, applies) into
    /// per-shape EWMA factors; `estimate_node_output_rows` consults these
    /// factors so all cost-based decisions consume corrected row counts.
    cardinality_feedback: Arc<CardinalityFeedbackManager>,
    /// Shared decision-level feedback store (Apply vs Join, phase 3).
    ///
    /// Records which decorrelation path actually executed and its measured
    /// rows / time; the CBO unnesting decision consults the empirical advice.
    decision_feedback: Arc<DecisionFeedbackStore>,
    /// Trigger for when the feedback history should be folded into
    /// `selectivity_feedback` (cooldown + error threshold).
    feedback_trigger: AutoFeedbackTrigger,
    /// Master switch for the feedback correction loop.
    enable_feedback: bool,
    /// Cross-query adaptive policy for the typed columnar chunk layout.
    ///
    /// Injected into every execution runtime; each query merges its columnar
    /// hit/miss counts back into this policy at completion, so the typed
    /// columnar path is gated by learned hit rate instead of a static switch.
    columnar_policy: Arc<crate::query::executor::streaming::chunk::ColumnarPolicy>,
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

        // Create a CTE (Common Table Expression) for cache manager management.
        let cte_cache_manager = Arc::new(CteCacheManager::new());

        Self::with_components(
            expression_context,
            stats_manager,
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
        cte_cache_manager: Arc<CteCacheManager>,
        cost_config: CostModelConfig,
    ) -> Self {
        // Create a cost calculator and a selective estimator.
        let cost_calculator = Arc::new(CostCalculator::with_config(
            stats_manager.clone(),
            cost_config,
        ));
        let selectivity_feedback = Arc::new(SelectivityFeedbackManager::new());
        let selectivity_estimator = Arc::new(SelectivityEstimator::with_feedback(
            stats_manager.clone(),
            selectivity_feedback.clone(),
        ));
        let cardinality_feedback = Arc::new(CardinalityFeedbackManager::new());
        let decision_feedback = Arc::new(DecisionFeedbackStore::new());

        // Create batch plan analyzer (unified analysis)
        let batch_plan_analyzer = BatchPlanAnalyzer::new();

        // Create a subquery to de-associate the optimizer.
        let subquery_unnesting_optimizer = SubqueryUnnestingOptimizer::new();

        // Create the heuristic batch optimizer (production heuristic main chain)
        let heuristic_batch = BatchOptimizer::from_registry(RuleRegistry::default());

        Self {
            expression_context,
            stats_manager,
            cte_cache_manager,
            cost_calculator,
            selectivity_estimator,
            batch_plan_analyzer,
            subquery_unnesting_optimizer,
            cost_config,
            heuristic_batch,
            last_batch_statistics: Mutex::new(Vec::new()),
            partitioning_planner: PartitioningPlanner::new(PartitioningConfig::default()),
            enable_heuristic: true,
            max_heuristic_iterations: 100,
            feedback_history: Arc::new(QueryFeedbackHistory::default()),
            selectivity_feedback,
            cardinality_feedback,
            decision_feedback,
            feedback_trigger: AutoFeedbackTrigger::default(),
            enable_feedback: true,
            columnar_policy: Arc::new(
                crate::query::executor::streaming::chunk::ColumnarPolicy::default(),
            ),
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

    /// Obtain the context of the expression.
    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expression_context
    }

    /// Obtain the shared query feedback history (stats feedback loop).
    ///
    /// Executions write estimated-vs-actual feedback here; selectivity
    /// auto-correction consumes it.  The history is process-shared across all
    /// queries running through this engine.
    pub fn feedback_history(&self) -> Arc<QueryFeedbackHistory> {
        Arc::clone(&self.feedback_history)
    }

    /// Obtain the shared selectivity correction manager (feedback loop).
    pub fn selectivity_feedback(&self) -> &Arc<SelectivityFeedbackManager> {
        &self.selectivity_feedback
    }

    /// Obtain the shared cardinality correction manager (feedback loop).
    pub fn cardinality_feedback(&self) -> &Arc<CardinalityFeedbackManager> {
        &self.cardinality_feedback
    }

    /// Obtain the shared decision feedback store (feedback loop).
    pub fn decision_feedback(&self) -> &Arc<DecisionFeedbackStore> {
        &self.decision_feedback
    }

    /// Enable / disable the feedback-driven selectivity correction loop.
    pub fn set_enable_feedback(&mut self, enable: bool) {
        self.enable_feedback = enable;
        log::info!(
            "Feedback-driven selectivity correction has been {}",
            if enable { "enabled" } else { "disabled" }
        );
    }

    /// Whether the feedback-driven selectivity correction loop is enabled.
    pub fn feedback_enabled(&self) -> bool {
        self.enable_feedback
    }

    /// Obtain the shared columnar layout policy.
    ///
    /// The policy is injected into every execution runtime; per-query
    /// columnar hit/miss counts are merged back at query completion.
    pub fn columnar_policy(&self) -> Arc<crate::query::executor::streaming::chunk::ColumnarPolicy> {
        Arc::clone(&self.columnar_policy)
    }

    /// Fold recorded execution feedback into the selectivity corrections.
    ///
    /// Iterates every fingerprint with feedback history; when the average
    /// row-estimation error passes the trigger threshold, the per-operator
    /// estimated-vs-actual ratios are smoothed into the shared
    /// [`SelectivityFeedbackManager`] (per normalized predicate key) and the
    /// shared [`CardinalityFeedbackManager`] (per normalized operator shape
    /// key).  Apply / SemiJoin executions are additionally folded into the
    /// [`DecisionFeedbackStore`] so the unnesting decision can use measured
    /// evidence.
    ///
    /// This is called at the start of [`OptimizeEngine::optimize`] and is
    /// gated by `enable_feedback`, so the hot path only pays for the RwLock
    /// reads of the history.
    pub fn maybe_apply_feedback(&self) {
        if !self.enable_feedback {
            return;
        }
        let history = self.feedback_history();
        let mut applied = 0usize;
        let mut decision_runs = 0usize;
        for fingerprint in history.get_all_fingerprints() {
            let Some(avg_error) = history.get_avg_row_error(&fingerprint) else {
                continue;
            };
            if !self.feedback_trigger.should_trigger(avg_error) {
                continue;
            }
            for feedback in history.get_feedback_for_query(&fingerprint) {
                let space = feedback.space.as_deref().unwrap_or("");
                if feedback.apply_rows > 0 {
                    self.decision_feedback.record_apply_run(
                        space,
                        feedback.apply_rows,
                        feedback.apply_time_us,
                    );
                    decision_runs += 1;
                }
                if feedback.join_rows > 0 {
                    self.decision_feedback.record_join_run(
                        space,
                        feedback.join_rows,
                        feedback.join_time_us,
                    );
                    decision_runs += 1;
                }
                for op in &feedback.operator_feedbacks {
                    if let Some(key) = &op.condition_key {
                        // Correction factor = actual_rows / estimated_rows.
                        let ratio = op.actual_rows as f64 / op.estimated_rows.max(1) as f64;
                        if self.selectivity_feedback.update_feedback_ratio(key, ratio) {
                            applied += 1;
                        }
                    } else if let Some(key) = &op.shape_key {
                        let ratio = op.actual_rows as f64 / op.estimated_rows.max(1) as f64;
                        if self.cardinality_feedback.update_feedback_ratio(key, ratio) {
                            applied += 1;
                        }
                    }
                }
            }
            self.feedback_trigger.mark_updated();
        }
        if applied > 0 {
            log::debug!(
                "Feedback loop: applied {} selectivity/cardinality corrections from {} fingerprints",
                applied,
                history.query_count()
            );
        }
        if decision_runs > 0 {
            log::debug!(
                "Feedback loop: folded {} Apply/Join decision runs from {} fingerprints",
                decision_runs,
                history.query_count()
            );
        }
    }

    /// Drop all feedback corrections scoped to `space` (`None` clears all).
    ///
    /// Called when a space's statistics are invalidated (ANALYZE force or
    /// DDL commit) so stale corrections cannot mislead estimates after a
    /// schema or data change.
    pub fn invalidate_space_feedback(&self, space: Option<&str>) {
        let removed = match space {
            Some(space) => {
                let removed = self
                    .selectivity_feedback
                    .remove_feedback_by_space(&format!("{}:", space));
                removed
                    + self
                        .cardinality_feedback
                        .remove_feedback_by_space(&format!("{}:", space))
            }
            None => {
                let keys = self.selectivity_feedback.get_all_keys();
                for key in &keys {
                    self.selectivity_feedback.remove_feedback(key);
                }
                let cardinality_keys = self.cardinality_feedback.get_all_keys();
                for key in &cardinality_keys {
                    self.cardinality_feedback.remove_feedback(key);
                }
                keys.len() + cardinality_keys.len()
            }
        };
        self.decision_feedback.invalidate_space(space);
        if removed > 0 {
            log::info!(
                "Invalidated {} feedback corrections for space {:?}",
                removed,
                space
            );
        }
    }

    /// Obtain batch plan analyzer
    pub fn batch_plan_analyzer(&self) -> &BatchPlanAnalyzer {
        &self.batch_plan_analyzer
    }

    /// Obtaining the subquery to de-associate the optimizer
    pub fn subquery_unnesting_optimizer(&self) -> &SubqueryUnnestingOptimizer {
        &self.subquery_unnesting_optimizer
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
        // Re-create batch plan analyzer
        self.batch_plan_analyzer = BatchPlanAnalyzer::new();
        // Re-create the subquery to de-associate the optimizer.
        self.subquery_unnesting_optimizer = SubqueryUnnestingOptimizer::new();
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
    /// `space`: The space of the query being optimized; `None` disables
    /// statistics-driven cost-based decisions.
    ///
    /// # Returns
    /// The optimized execution plan
    pub fn optimize(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
    ) -> OptimizeResult<ExecutionPlan> {
        // Without storage layout information the planner falls back to the
        // configured (or absent) vertex-id range — safe, no evidence.
        self.optimize_with_layout(plan, space, &PartitioningLayoutInfo::default())
    }

    /// Optimize with storage-provided layout information (layout version and
    /// self-proven vertex-id domain). See [`PartitioningLayoutInfo`].
    pub fn optimize_with_layout(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
        layout: &PartitioningLayoutInfo,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut current_plan = plan;

        // Phase 0: fold recorded execution feedback into the selectivity
        // corrections (stats feedback loop).  Gated by
        // `enable_feedback`; cheap when no history is present.
        self.maybe_apply_feedback();

        // Phase 1: Heuristic Optimization (Always Executed)
        if self.enable_heuristic {
            log::debug!("Starting Phase 1: Heuristic Optimization");
            current_plan = self
                .apply_heuristic_with_max_iterations(current_plan, self.max_heuristic_iterations)?;
            log::debug!("Phase 1 completed successfully");
        }

        // Phase 2: Cost-Based Optimization (always active — conservative rules)
        log::debug!("Starting Phase 2: Cost-Based Optimization");
        current_plan = self.apply_cost_based(current_plan, space)?;
        log::debug!("Phase 2 completed successfully");

        current_plan = self.apply_partitioning_selection(current_plan, space, layout);

        Ok(current_plan)
    }

    fn apply_partitioning_selection(
        &self,
        mut plan: ExecutionPlan,
        space: Option<&str>,
        layout: &PartitioningLayoutInfo,
    ) -> ExecutionPlan {
        if plan.partition_spec().is_some() {
            return plan;
        }
        let Some(root) = plan.root.as_ref() else {
            return plan;
        };
        let stats = StatsView::new(&self.stats_manager, space);
        let decision = self
            .partitioning_planner
            .decide_with_layout(root, &stats, layout);
        if let Some(spec) = decision.partition_spec {
            log::debug!("Selected partition layout: {}", decision.reason);
            plan.set_partition_spec(spec);
        } else if !decision.reason.is_empty() {
            // Keep the decision observable: EXPLAIN ANALYZE / PROFILE report
            // the reason whenever the plan falls back to serial execution.
            plan.parallel_fallback_reason = decision.reason;
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
        // Interior mutability via AtomicUsize: set_max_iterations does not need &mut self.
        self.heuristic_batch.set_max_iterations(max_iterations);

        let root = match plan.root.clone() {
            Some(root) => root,
            None => return Ok(plan),
        };
        let result = self
            .heuristic_batch
            .optimize(root)
            .map_err(|e| OptimizeError::HeuristicFailed(e.to_string()))?;
        if let Ok(mut guard) = self.last_batch_statistics.lock() {
            *guard = result.batch_statistics.clone();
        }
        let mut new_plan = plan;
        new_plan.set_root(result.optimized_plan);
        Ok(new_plan)
    }

    /// Apply cost-based optimization strategies.
    ///
    /// The decision phases (join order, index selection, aggregate strategy)
    /// consume the pure logical plan when one is attached to the execution
    /// plan (`optimize_logical`); otherwise the physical root is optimized
    /// directly (`optimize_plan_nodes`). Structural rewrites always operate
    /// on the physical root because the physical plan is the artifact
    /// consumed by the physical planner.
    fn apply_cost_based(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut plan = plan;
        let stats = StatsView::new(&self.stats_manager, space);

        let logical = plan.logical_plan().cloned();
        match logical {
            Some(logical) => self.optimize_logical(&logical, &stats, space, &mut plan)?,
            None => self.optimize_plan_nodes(&stats, space, &mut plan)?,
        }
        Ok(plan)
    }

    /// Cost-based decisions driven by the logical plan tree.
    ///
    /// The logical tree (attached during planning, before heuristic
    /// rewrites) is the decision fact source for join order, index
    /// selection, and aggregate strategy. Decision notes are recorded from
    /// the logical walkers; the structural rewrites (join reorder →
    /// InnerJoin, ScanVertices → IndexScan, Sort+Limit → TopN) are
    /// applied to the physical root, which is what the physical planner
    /// executes.
    fn optimize_logical(
        &self,
        logical: &LogicalPlan,
        stats: &StatsView,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) -> OptimizeResult<()> {
        // Phase 1: Subquery unnesting (structural rewrite on the physical
        // root; the logical tree cannot represent PatternApply, so plans
        // containing one fall back to `optimize_plan_nodes` — defensive).
        self.apply_unnesting(plan, stats);

        // Phase 2: Join order — decision on the logical tree, rewrite on
        // the physical root.
        self.apply_join_order_logical(logical, stats, plan);

        // Phase 3: Cost-based index selection — decision on the logical
        // tree, rewrite on the physical root.
        self.apply_index_selection_logical(logical, space, plan);

        // Phase 4: Sort + Limit → TopN conversion (residual patterns,
        // cost-based; physical structural rewrite).
        self.apply_topn_wiring(plan, stats);

        // Phase 5: Aggregate strategy selection — decision on the logical
        // tree (the strategy is consumed by the physical planner via the
        // notes).
        self.apply_aggregate_strategy_logical(logical, stats, plan);

        // Phase 6: Collect per-node row estimates for estimated_rows writeback.
        self.apply_row_estimates(plan, stats);

        // Phase 7: Expression precomputation decisions (note-only; EXPLAIN
        // observability for expressions worth precomputing).
        self.apply_precompute_notes(plan);

        Ok(())
    }

    /// Cost-based optimization on the physical root directly.
    ///
    /// Fallback path used when no logical plan is attached (DDL/DML
    /// statements, or operator trees the physical-to-logical converter does
    /// not support yet).
    fn optimize_plan_nodes(
        &self,
        stats: &StatsView,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) -> OptimizeResult<()> {
        // Phase 1: Subquery unnesting (PatternApply → InnerJoin)
        self.apply_unnesting(plan, stats);

        // Phase 2: Join order optimization
        if let Some(ref root) = plan.root.clone() {
            let mut notes = Vec::new();
            let mut decisions = std::collections::HashMap::new();
            let rewritten = crate::query::optimizer::cost_based::join_order_rewriter::
                walk_and_optimize_joins_with_decisions(
                    root,
                    stats,
                    &self.cost_calculator,
                    &mut notes,
                    &mut Some(&mut decisions),
                );
            plan.set_root(rewritten);
            plan.join_algorithms = decisions;
            plan.cbo_notes.extend(notes);
        }

        // Phase 3: Cost-based index selection (ScanVertices → IndexScan)
        if let Some(ref root) = plan.root.clone() {
            let selector = IndexSelector::new(
                self.cost_calculator.clone(),
                self.selectivity_estimator.clone(),
            );
            let mut notes = Vec::new();
            let rewritten =
                crate::query::optimizer::cost_based::index_selection::rewrite_index_scans(
                    root,
                    &selector,
                    &self.stats_manager,
                    space,
                    &mut notes,
                );
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Phase 4: Sort + Limit → TopN conversion (residual patterns)
        self.apply_topn_wiring(plan, stats);

        // Phase 5: Aggregate strategy selection (decision notes)
        if let Some(ref root) = plan.root.clone() {
            let selector = AggregateStrategySelector::new(self.cost_calculator.clone());
            let mut notes = Vec::new();
            let rewritten = self.select_aggregate_strategies(root, stats, &selector, &mut notes);
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Phase 6: Collect per-node row estimates for estimated_rows writeback.
        self.apply_row_estimates(plan, stats);

        // Phase 7: Expression precomputation decisions (note-only)
        self.apply_precompute_notes(plan);

        Ok(())
    }

    /// Subquery unnesting: PatternApply → InnerJoin when cost-beneficial.
    fn apply_unnesting(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(ref root) = plan.root.clone() {
            let mut notes = Vec::new();
            let rewritten = self.unnest_subqueries(root, stats, &mut notes);
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }
    }

    /// Join order decision on the logical tree; structural rewrite applied
    /// to the physical root.
    fn apply_join_order_logical(
        &self,
        logical: &LogicalPlan,
        stats: &StatsView,
        plan: &mut ExecutionPlan,
    ) {
        use crate::query::optimizer::cost_based::join_order_rewriter::walk_and_optimize_joins_logical;

        // Decision on the logical tree.
        let mut notes = Vec::new();
        let rewritten_logical = walk_and_optimize_joins_logical(
            logical.root(),
            stats,
            &self.cost_calculator,
            &mut notes,
        );
        plan.cbo_notes.extend(notes);

        // Keep the attached logical plan in sync with the join order decision.
        let mut updated_logical = logical.clone();
        updated_logical.root = rewritten_logical;
        plan.set_logical_plan(updated_logical);

        // Structural rewrite on the physical root. The physical walker
        // recomputes the same decision; its notes are discarded here because
        // the logical walker is the note source.
        if let Some(ref root) = plan.root.clone() {
            let mut scratch = Vec::new();
            let mut decisions = std::collections::HashMap::new();
            let rewritten = crate::query::optimizer::cost_based::join_order_rewriter::
                walk_and_optimize_joins_with_decisions(
                    root,
                    stats,
                    &self.cost_calculator,
                    &mut scratch,
                    &mut Some(&mut decisions),
                );
            plan.set_root(rewritten);
            plan.join_algorithms = decisions;
        }
    }

    /// Index selection decision on the logical tree; structural rewrite
    /// applied to the physical root.
    fn apply_index_selection_logical(
        &self,
        logical: &LogicalPlan,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) {
        use crate::query::optimizer::cost_based::index_selection::rewrite_index_scans_logical;

        let selector = IndexSelector::new(
            self.cost_calculator.clone(),
            self.selectivity_estimator.clone(),
        );

        // Decision on the logical tree.
        let mut notes = Vec::new();
        rewrite_index_scans_logical(
            logical.root(),
            &selector,
            &self.stats_manager,
            space,
            &mut notes,
        );
        plan.cbo_notes.extend(notes);

        // Structural rewrite on the physical root (notes recomputed there
        // are discarded — the logical walker is the note source).
        if let Some(ref root) = plan.root.clone() {
            let mut scratch = Vec::new();
            let rewritten =
                crate::query::optimizer::cost_based::index_selection::rewrite_index_scans(
                    root,
                    &selector,
                    &self.stats_manager,
                    space,
                    &mut scratch,
                );
            plan.set_root(rewritten);
        }
    }

    /// Aggregate strategy decision on the logical tree (note-only).
    fn apply_aggregate_strategy_logical(
        &self,
        logical: &LogicalPlan,
        stats: &StatsView,
        plan: &mut ExecutionPlan,
    ) {
        use crate::query::optimizer::cost_based::aggregate_strategy::walk_aggregate_strategies_logical;

        let selector = AggregateStrategySelector::new(self.cost_calculator.clone());
        let mut notes = Vec::new();
        walk_aggregate_strategies_logical(
            logical.root(),
            stats,
            &selector,
            &self.selectivity_estimator,
            &mut notes,
        );
        plan.cbo_notes.extend(notes);
    }

    /// Sort + Limit → TopN conversion (residual patterns, cost-based).
    fn apply_topn_wiring(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(ref root) = plan.root.clone() {
            let optimizer = SortEliminationOptimizer::new(self.cost_calculator.clone());
            let mut notes = Vec::new();
            let rewritten =
                crate::query::optimizer::cost_based::topn_wiring::rewrite_sort_with_limits(
                    root,
                    &optimizer,
                    stats,
                    &self.selectivity_estimator,
                    &self.cardinality_feedback,
                    &mut notes,
                );
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }
    }

    /// Collect per-node row estimates for estimated_rows writeback.
    fn apply_row_estimates(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(ref root) = plan.root.clone() {
            plan.row_estimates =
                crate::query::optimizer::cost_based::row_estimates::collect_node_row_estimates(
                    root,
                    stats,
                    &self.selectivity_estimator,
                );
        }
    }

    /// Expression precomputation decisions (note-only; EXPLAIN observability
    /// for expressions worth precomputing).
    fn apply_precompute_notes(&self, plan: &mut ExecutionPlan) {
        if let Some(ref root) = plan.root.clone() {
            let optimizer = crate::query::optimizer::cost_based::expression_precomputation::ExpressionPrecomputationOptimizer::new(self.cost_calculator.clone());
            let notes =
                crate::query::optimizer::cost_based::precomputation_wiring::collect_precompute_notes(
                    root,
                    &optimizer,
                );
            plan.cbo_notes.extend(notes);
        }
    }

    /// Walk the plan and record the aggregation strategy decision for every
    /// aggregate node as a CBO note (cost-driven, observable in EXPLAIN).
    fn select_aggregate_strategies(
        &self,
        node: &PlanNodeEnum,
        stats: &StatsView,
        selector: &AggregateStrategySelector,
        notes: &mut Vec<String>,
    ) -> PlanNodeEnum {
        use crate::query::optimizer::cost_based::row_estimates::estimate_node_output_rows;
        use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
        use PlanNodeEnum::*;

        if let Aggregate(aggregate) = node {
            let input_rows =
                estimate_node_output_rows(aggregate.input(), stats, &self.selectivity_estimator);
            let context = AggregateContext {
                input_rows,
                group_keys: aggregate.group_keys().to_vec(),
                agg_function_count: aggregate.aggregation_functions().len(),
                memory_limit: 0,
                input_is_sorted: false,
                sort_keys_match_group_keys: false,
                is_deterministic: true,
                complexity_score: 0,
                table_name: None,
            };
            let decision = selector.select_strategy(stats.space().unwrap_or(""), &context);
            notes.push(format!(
                "aggregate: strategy={:?} (reason={:?}, est_rows={})",
                decision.strategy, decision.reason, decision.estimated_output_rows
            ));
        }

        // Recursively rewrite children.
        let mut closure =
            |child: &PlanNodeEnum| self.select_aggregate_strategies(child, stats, selector, notes);
        crate::query::optimizer::cost_based::traversal::rewrite_children(node, &mut closure)
    }

    /// Recursively walk the plan tree and rewrite PatternApply → InnerJoin
    /// when the subquery unnesting optimizer determines it is beneficial.
    fn unnest_subqueries(
        &self,
        node: &PlanNodeEnum,
        stats: &StatsView,
        notes: &mut Vec<String>,
    ) -> PlanNodeEnum {
        use PlanNodeEnum::*;

        // Try PatternApply unnesting at this level first.
        if let PatternApply(apply) = node {
            let analysis = self.batch_plan_analyzer.analyze(node);
            let advice = self.decision_feedback.advice(stats.space().unwrap_or(""));
            if let UnnestDecision::ShouldUnnest { ref reason, .. } =
                self.subquery_unnesting_optimizer.should_unnest(
                    apply,
                    &analysis,
                    stats,
                    &self.selectivity_estimator,
                    &self.cardinality_feedback,
                    &advice,
                )
            {
                log::debug!("CBO: unnesting PatternApply -> SemiJoin ({:?})", reason);
                notes.push(format!("unnest pattern_apply -> semi_join ({:?})", reason));
                if let Ok(join) = self.subquery_unnesting_optimizer.unnest(apply.clone()) {
                    return self.unnest_subqueries(&join, stats, notes);
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
                let new_input = self.unnest_subqueries(cloned.input(), stats, notes);
                cloned.set_input(new_input);
                cloned
            }};
        }
        macro_rules! rewrite_binary {
            ($n:expr) => {{
                let mut cloned = $n.clone();
                let new_left = self.unnest_subqueries(cloned.left_input(), stats, notes);
                let new_right = self.unnest_subqueries(cloned.right_input(), stats, notes);
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

            // PatternApply: unnesting was attempted above; if we reach here
            // the decision was to keep it, so rewrite the left child (the
            // main data pipeline). The right child (subquery pattern) is
            // typically a leaf scan and is left unchanged.
            PatternApply(n) => PatternApply(rewrite_single!(n)),

            // CorrelatedApply: the right subtree is re-executed per row and
            // is never unnest-ed; rewrite only the outer (left) pipeline.
            CorrelatedApply(n) => CorrelatedApply(rewrite_single!(n)),

            // Leaf / unsupported nodes: return unchanged.
            _ => node.clone(),
        }
    }

    /// Get the heuristic batch optimizer
    pub fn heuristic_batch(&self) -> &BatchOptimizer {
        &self.heuristic_batch
    }

    /// Get the batch statistics of the last heuristic optimization (for EXPLAIN diagnostics)
    pub fn last_batch_statistics(&self) -> Vec<(OptimizationBatch, BatchStatistics)> {
        match self.last_batch_statistics.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
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
    fn test_feedback_loop_corrects_selectivity() {
        use crate::core::types::Expression;
        use crate::query::optimizer::cost::selectivity::condition_key;
        use crate::query::optimizer::stats::feedback::query::{
            OperatorFeedback, QueryExecutionFeedback,
        };

        let engine = OptimizerEngine::default();

        // Register the condition through the estimator (as optimization does).
        let expr = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("v".to_string())),
                property: "age".to_string(),
            }),
            op: crate::core::types::BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(crate::core::Value::Int(30))),
        };
        let original =
            engine
                .selectivity_estimator
                .estimate_from_expression(Some("test_space"), &expr, None);
        let key = condition_key(Some("test_space"), &expr);
        assert!(engine
            .selectivity_feedback
            .get_corrected_selectivity(&key)
            .is_some());

        // Simulate history: filter estimated 100 rows, actually produced 10.
        // Repeat several times so the EWMA correction converges toward 0.1.
        for _ in 0..8 {
            let mut feedback = QueryExecutionFeedback::new("fp".to_string());
            feedback.space = Some("test_space".to_string());
            feedback.add_operator_feedback(OperatorFeedback {
                operator_id: "op1".to_string(),
                operator_type: "Filter".to_string(),
                estimated_rows: 100,
                actual_rows: 10,
                estimated_time_us: 0,
                actual_time_us: 0,
                execution_loops: 1,
                condition_key: Some(key.clone()),
                shape_key: None,
            });
            engine.feedback_history.add_feedback(feedback);
        }

        engine.maybe_apply_feedback();

        let corrected = engine
            .selectivity_feedback
            .get_corrected_selectivity(&key)
            .expect("condition should remain registered");
        // actual/estimated = 0.1, so the corrected selectivity must converge
        // toward 10% of the original estimate.
        assert!(
            corrected < original * 0.35,
            "corrected={} should be far below original={}",
            corrected,
            original
        );

        // Invalidation: corrections for the space are dropped.
        engine.invalidate_space_feedback(Some("test_space"));
        assert!(engine
            .selectivity_feedback
            .get_corrected_selectivity(&key)
            .is_none());
    }

    #[test]
    fn test_feedback_loop_respects_enable_switch() {
        use crate::query::optimizer::stats::feedback::query::{
            OperatorFeedback, QueryExecutionFeedback,
        };

        let mut engine = OptimizerEngine::default();
        engine.set_enable_feedback(false);

        let mut feedback = QueryExecutionFeedback::new("fp".to_string());
        feedback.add_operator_feedback(OperatorFeedback {
            operator_id: "op1".to_string(),
            operator_type: "Filter".to_string(),
            estimated_rows: 100,
            actual_rows: 10,
            estimated_time_us: 0,
            actual_time_us: 0,
            execution_loops: 1,
            condition_key: Some("space:v.age > 30".to_string()),
            shape_key: None,
        });
        engine.feedback_history.add_feedback(feedback);

        engine.maybe_apply_feedback();
        // No correction applied: the key was never registered by an estimate.
        assert!(engine
            .selectivity_feedback
            .get_corrected_selectivity("space:v.age > 30")
            .is_none());
        // Nothing was applied because the switch is off; enabling it and
        // triggering with a registered key would apply corrections.
        assert!(!engine.feedback_enabled());
    }

    #[test]
    fn test_feedback_loop_corrects_cardinality_shape_keys() {
        use crate::query::optimizer::stats::feedback::query::{
            OperatorFeedback, QueryExecutionFeedback,
        };

        let engine = OptimizerEngine::default();

        // Register the shape through the estimate path (as optimization does).
        let key = "test_space:ScanVertices:person".to_string();
        engine.cardinality_feedback.register_key(key.clone(), 100.0);

        // Simulate history: the scan estimated 100 rows, actually produced 300.
        for _ in 0..10 {
            let mut feedback = QueryExecutionFeedback::new("fp".to_string());
            feedback.space = Some("test_space".to_string());
            feedback.add_operator_feedback(OperatorFeedback {
                operator_id: "op1".to_string(),
                operator_type: "ScanVertices".to_string(),
                estimated_rows: 100,
                actual_rows: 300,
                estimated_time_us: 0,
                actual_time_us: 0,
                execution_loops: 1,
                condition_key: None,
                shape_key: Some(key.clone()),
            });
            engine.feedback_history.add_feedback(feedback);
        }

        engine.maybe_apply_feedback();

        let corrected = engine
            .cardinality_feedback
            .corrected_rows(&key)
            .expect("shape should remain registered");
        assert!(
            corrected > 150.0,
            "corrected={} should move toward 300",
            corrected
        );

        // Decision feedback: an Apply run with rows and time is folded in.
        let mut feedback = QueryExecutionFeedback::new("fp2".to_string());
        feedback.space = Some("test_space".to_string());
        feedback.apply_rows = 500;
        feedback.apply_time_us = 50_000;
        engine.feedback_history.add_feedback(feedback);
        engine.maybe_apply_feedback();
        let advice = engine.decision_feedback.advice("test_space");
        assert!(advice.apply_cost_per_row.is_some());

        // Invalidation drops both the shape corrections and decision stats.
        engine.invalidate_space_feedback(Some("test_space"));
        assert!(engine.cardinality_feedback.corrected_rows(&key).is_none());
        let advice = engine.decision_feedback.advice("test_space");
        assert!(advice.apply_cost_per_row.is_none());
    }

    #[test]
    fn test_optimizer_engine_max_iterations() {
        let mut engine = OptimizerEngine::default();

        engine.set_max_heuristic_iterations(50);
        assert_eq!(engine.max_heuristic_iterations, 50);
    }

    #[test]
    fn cost_based_phases_rewrite_and_emit_notes() {
        use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use crate::core::types::{Index, IndexStatus, IndexType};
        use crate::core::Value;
        use crate::query::optimizer::stats::TagStatistics;
        use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
        use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
        use crate::query::planning::plan::core::nodes::operation::sort_node::{
            LimitNode, SortItem, SortNode,
        };
        use std::sync::Arc;

        let engine = OptimizerEngine::default();

        // Register index + vertex statistics for the tag so cost-based
        // index selection and the row estimates are data-driven.
        engine.stats_manager().register_tag_indexes(
            "test",
            "person",
            7,
            vec![Index {
                id: 3,
                name: "idx_person_name".to_string(),
                space_id: 1,
                schema_name: "person".to_string(),
                fields: Vec::new(),
                properties: vec!["name".to_string()],
                index_type: IndexType::TagIndex,
                status: IndexStatus::Active,
                is_unique: false,
                comment: None,
                covering: false,
                partial_condition: None,
            }],
        );
        let mut tag_stats = TagStatistics::new("person".to_string());
        tag_stats.vertex_count = 10_000;
        engine.stats_manager().update_tag_stats("test", tag_stats);

        // Build: Limit -> Sort -> Filter(ScanVertices(person, name = 'alice')).
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        scan.set_col_names(vec!["n".to_string()]);
        scan.set_output_var("n".to_string());
        let context = Arc::new(ExpressionAnalysisContext::new());
        let predicate = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "name".to_string(),
            }),
            op: crate::core::types::operators::BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::String("alice".into()))),
        };
        let id = context.register_expression(ExpressionMeta::new(predicate));
        let filter = FilterNode::new(
            PlanNodeEnum::ScanVertices(scan),
            ContextualExpression::new(id, context),
        )
        .expect("filter should build");
        let sort = SortNode::new(
            PlanNodeEnum::Filter(filter),
            vec![SortItem::column_asc("n.name".to_string())],
        )
        .expect("sort should build");
        let limit = LimitNode::new(PlanNodeEnum::Sort(sort), 0, 50).expect("limit should build");
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::Limit(limit)));

        let optimized = engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");

        // The heuristic phase converts Limit(offset=0) -> Sort to TopN, so
        // the plan must contain a TopN and an IndexScan for the predicate.
        let root = optimized.root.as_ref().expect("root should exist");
        assert!(
            contains_variant(root, &|node| matches!(node, PlanNodeEnum::TopN(_))),
            "expected TopN in optimized plan"
        );
        assert!(
            contains_variant(root, &|node| matches!(node, PlanNodeEnum::IndexScan(_))),
            "expected IndexScan in optimized plan"
        );

        // Decision notes and row estimates must be produced.
        assert!(optimized
            .cbo_notes
            .iter()
            .any(|note| note.starts_with("index:")));
        assert!(!optimized.row_estimates.is_empty());

        // The filter remains above the index scan (residual predicate).
        assert!(contains_variant(root, &|node| matches!(
            node,
            PlanNodeEnum::Filter(_)
        )));
    }

    fn contains_variant(node: &PlanNodeEnum, predicate: &dyn Fn(&PlanNodeEnum) -> bool) -> bool {
        if predicate(node) {
            return true;
        }
        node.children()
            .iter()
            .any(|child| contains_variant(child, predicate))
    }

    #[test]
    fn precompute_notes_emitted_for_reused_expressions() {
        use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use crate::core::types::operators::BinaryOperator;
        use crate::core::Value;
        use crate::core::YieldColumn;
        use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
        use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
        use std::sync::Arc;

        let mut engine = OptimizerEngine::default();
        // Keep the plan shape untouched so duplicate projection columns
        // survive (heuristic dedup would collapse them).
        engine.set_enable_heuristic(false);

        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        scan.set_col_names(vec!["n".to_string()]);
        scan.set_output_var("n".to_string());

        // (a + b) * 2: complex enough to clear the precomputation cost floor.
        let expr = Expression::Binary {
            left: Box::new(Expression::Binary {
                left: Box::new(Expression::Variable("a".to_string())),
                op: BinaryOperator::Add,
                right: Box::new(Expression::Variable("b".to_string())),
            }),
            op: BinaryOperator::Multiply,
            right: Box::new(Expression::Literal(Value::Int(2))),
        };
        let context = Arc::new(ExpressionAnalysisContext::new());
        let id = context.register_expression(ExpressionMeta::new(expr));
        let contextual = ContextualExpression::new(id, context);

        // The same expression is referenced by three projection columns.
        let columns: Vec<YieldColumn> = (0..3)
            .map(|i| YieldColumn {
                expression: contextual.clone(),
                alias: format!("c{}", i),
                is_matched: false,
            })
            .collect();
        let project = ProjectNode::new(PlanNodeEnum::ScanVertices(scan), columns)
            .expect("project should build");
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::Project(project)));

        let optimized = engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");

        assert!(
            optimized
                .cbo_notes
                .iter()
                .any(|note| note.starts_with("precompute:")),
            "expected precompute decision notes, got: {:?}",
            optimized.cbo_notes
        );
    }

    #[test]
    fn cost_based_consumes_logical_plan_when_attached() {
        use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use crate::core::types::{Index, IndexStatus, IndexType};
        use crate::core::Value;
        use crate::query::optimizer::stats::TagStatistics;
        use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
        use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
        use crate::query::planning::plan::core::nodes::operation::sort_node::{
            LimitNode, SortItem, SortNode,
        };
        use crate::query::planning::plan::logical_plan::LogicalPlan;
        use std::sync::Arc;

        let engine = OptimizerEngine::default();

        // Register index + vertex statistics so the index selection decision
        // is data-driven.
        engine.stats_manager().register_tag_indexes(
            "test",
            "person",
            7,
            vec![Index {
                id: 3,
                name: "idx_person_name".to_string(),
                space_id: 1,
                schema_name: "person".to_string(),
                fields: Vec::new(),
                properties: vec!["name".to_string()],
                index_type: IndexType::TagIndex,
                status: IndexStatus::Active,
                is_unique: false,
                comment: None,
                covering: false,
                partial_condition: None,
            }],
        );
        let mut tag_stats = TagStatistics::new("person".to_string());
        tag_stats.vertex_count = 10_000;
        engine.stats_manager().update_tag_stats("test", tag_stats);

        // Build: Limit -> Sort -> Filter(ScanVertices(person, name = 'alice')).
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        scan.set_col_names(vec!["n".to_string()]);
        scan.set_output_var("n".to_string());
        let context = Arc::new(ExpressionAnalysisContext::new());
        let predicate = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "name".to_string(),
            }),
            op: crate::core::types::operators::BinaryOperator::Equal,
            right: Box::new(Expression::Literal(Value::String("alice".into()))),
        };
        let id = context.register_expression(ExpressionMeta::new(predicate));
        let filter = FilterNode::new(
            PlanNodeEnum::ScanVertices(scan),
            ContextualExpression::new(id, context),
        )
        .expect("filter should build");
        let sort = SortNode::new(
            PlanNodeEnum::Filter(filter),
            vec![SortItem::column_asc("n.name".to_string())],
        )
        .expect("sort should build");
        let limit = LimitNode::new(PlanNodeEnum::Sort(sort), 0, 50).expect("limit should build");
        let root = PlanNodeEnum::Limit(limit);
        let mut plan = ExecutionPlan::new(Some(root.clone()));

        // Attach the pure logical plan — the CBO decision phases must
        // consume it.
        let logical_plan = LogicalPlan::from_plan_node(&root).expect("conversion should succeed");
        plan.set_logical_plan(logical_plan);

        let optimized = engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");

        // The structural rewrites still apply to the physical root.
        let root = optimized.root.as_ref().expect("root should exist");
        assert!(
            contains_variant(root, &|node| matches!(node, PlanNodeEnum::IndexScan(_))),
            "expected IndexScan in optimized plan"
        );
        assert!(
            contains_variant(root, &|node| matches!(node, PlanNodeEnum::TopN(_))),
            "expected TopN in optimized plan"
        );

        // The decision notes come from the logical walkers.
        assert!(
            optimized
                .cbo_notes
                .iter()
                .any(|note| note.starts_with("index:")),
            "expected index decision note from the logical walker, got: {:?}",
            optimized.cbo_notes
        );
        assert!(!optimized.row_estimates.is_empty());

        // The logical plan survives optimization (join order write-back).
        assert!(
            optimized.logical_plan().is_some(),
            "logical plan must remain attached after optimization"
        );
    }

    #[test]
    fn cost_based_falls_back_when_no_logical_plan_attached() {
        use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode;

        let mut engine = OptimizerEngine::default();
        engine.set_enable_heuristic(false);

        // Argument nodes are not supported by the physical-to-logical
        // converter, so no logical plan is attached and the physical
        // fallback path must keep the plan intact.
        let arg = ArgumentNode::new(-1, "x");
        let plan = ExecutionPlan::new(Some(PlanNodeEnum::Argument(arg)));
        assert!(plan.logical_plan().is_none());

        let optimized = engine
            .optimize(plan, Some("test"))
            .expect("optimization should succeed");
        assert!(matches!(optimized.root, Some(PlanNodeEnum::Argument(_))));
        assert!(optimized.logical_plan().is_none());
    }
}
