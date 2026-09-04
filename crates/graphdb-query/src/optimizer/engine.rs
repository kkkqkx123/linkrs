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
//! use graphdb_query::optimizer::cost::CostModelConfig;
//! use graphdb_query::optimizer::engine::OptimizerEngine;
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

use crate::optimizer::cost_based::subquery_unnesting::UnnestDecision;
use crate::optimizer::cost_based::{
    AggregateContext, AggregateStrategySelector, IndexSelector, SortEliminationOptimizer,
};
use crate::optimizer::heuristic::batch::{BatchOptimizer, BatchStatistics, OptimizationBatch};
use crate::optimizer::heuristic::rule_enum::RuleRegistry;
use crate::optimizer::heuristic::{LogicalBatchOptimizer, PhysicalHeuristicOptimizer};
use crate::optimizer::partitioning::{
    PartitioningConfig, PartitioningLayoutInfo, PartitioningPlanner,
};
use crate::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
use crate::optimizer::stats::feedback::decision::DecisionFeedbackStore;
use crate::optimizer::stats::feedback::history::QueryFeedbackHistory;
use crate::optimizer::stats::feedback::selectivity::SelectivityFeedbackManager;
use crate::optimizer::stats::feedback::trigger::AutoFeedbackTrigger;
use crate::optimizer::stats::StatsView;
use crate::optimizer::{
    BatchPlanAnalyzer, CostCalculator, CostModelConfig, CteCacheManager, SelectivityEstimator,
    StatisticsManager, SubqueryUnnestingOptimizer,
};
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;

mod feedback;
#[cfg(test)]
mod tests;

use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::ExecutionPlan;
use crate::planning::plan::PlanNodeEnum;

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
    /// Logical heuristic optimizer (operates on LogicalNodeEnum, pre-CBO)
    logical_heuristic: LogicalBatchOptimizer,
    /// Physical heuristic optimizer (operates on PlanNodeEnum, post-mapping)
    physical_heuristic: PhysicalHeuristicOptimizer,
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
    /// Shared decision-level feedback store (Apply vs Join decorrelation).
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
    columnar_policy: Arc<crate::executor::streaming::chunk::ColumnarPolicy>,
    /// Optional observability sink for factorization fallback counters.
    ///
    /// When set, the factorization steps increment the corresponding
    /// `MetricType` counters; when `None` (the default) no metrics work is
    /// performed and existing constructors behave unchanged.
    metrics_stats: Option<Arc<graphdb_core::stats::StatsManager>>,
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
            None,
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
        metrics_stats: Option<Arc<graphdb_core::stats::StatsManager>>,
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

        // Create the logical heuristic optimizer (logical tree, pre-CBO)
        let logical_heuristic = LogicalBatchOptimizer::new();

        // Create the physical heuristic optimizer (physical tree, post-mapping)
        let physical_heuristic = PhysicalHeuristicOptimizer::from_registry(RuleRegistry::default());

        Self {
            expression_context,
            stats_manager,
            cte_cache_manager,
            cost_calculator,
            selectivity_estimator,
            batch_plan_analyzer,
            subquery_unnesting_optimizer,
            cost_config,
            logical_heuristic,
            physical_heuristic,
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
            columnar_policy: Arc::new(crate::executor::streaming::chunk::ColumnarPolicy::default()),
            metrics_stats,
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
    pub fn columnar_policy(&self) -> Arc<crate::executor::streaming::chunk::ColumnarPolicy> {
        Arc::clone(&self.columnar_policy)
    }

    /// Attach the observability sink for factorization fallback counters.
    pub fn set_metrics_stats(&mut self, stats: Arc<graphdb_core::stats::StatsManager>) {
        self.metrics_stats = Some(stats);
    }

    /// Access the observability sink for factorization fallback counters.
    pub fn metrics_stats(&self) -> Option<Arc<graphdb_core::stats::StatsManager>> {
        self.metrics_stats.clone()
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
        stats_manager: Arc<graphdb_core::stats::StatsManager>,
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

        // Fold recorded execution feedback into the selectivity
        // corrections (stats feedback loop).  Gated by
        // `enable_feedback`; cheap when no history is present.
        self.maybe_apply_feedback();

        // Ensure a logical plan is available for the optimization pipeline.
        // Planners that natively emit a logical tree need no work here. The
        // reverse conversion below is the single compatibility bridge for
        // statements that are physical-only by design (remaining legacy
        // planners, administrative statements, search operators): when it
        // fails the plan keeps its physical shape and runs flat, with the
        // decision recorded in `cbo_notes` and the fallback counter.
        current_plan = self.ensure_logical_plan(current_plan);

        // Factorization: remove factorization before heuristic passes so they
        // operate on a flat view. Mirrors `RemoveFactorizationRewriter` at
        // `optimizer.cpp:1`.
        current_plan = self.apply_remove_factorization(current_plan);

        // Phase 1: Logical Heuristic (operates on LogicalNodeEnum).
        if self.enable_heuristic {
            log::debug!("Starting logical heuristic optimization");
            current_plan = self.apply_logical_heuristic(current_plan)?;
            log::debug!("Logical heuristic optimization completed successfully");
        }

        // Phase 2: CBO (decisions and rewrites on LogicalNodeEnum,
        // mirrored on the physical root; always active — conservative rules)
        log::debug!("Starting cost-based optimization");
        current_plan = self.apply_cost_based(current_plan, space)?;
        log::debug!("Cost-based optimization completed successfully");

        // Factorization: re-insert flatten operators after all optimizations.
        // Mirrors `FactorizationRewriter` at `optimizer.cpp:4`.
        current_plan = self.apply_factorization(current_plan);

        // Cost-based fallback: rewrite WCO intersect to hash join when the
        // hash join is cheaper, and record the decision in `cbo_notes`.
        current_plan = self.apply_intersect_to_join_rewrite(current_plan, space);

        // Phase 3: Physical Mapping (LogicalNodeEnum → PlanNodeEnum).
        // Full logical-to-physical conversion (introduces physical choices
        // such as IndexScan). The mapped tree carries factorization
        // operators and cost-based index hints; the merge below overlays the
        // physical choices the cost-based rewriters made directly on the
        // physical root (IndexScan limits, TopN), so neither side is lost.
        current_plan = self.apply_physical_mapping(current_plan);

        // Phase 4: Physical Heuristic (operates on PlanNodeEnum).
        if self.enable_heuristic {
            log::debug!("Starting physical heuristic optimization");
            current_plan =
                self.apply_physical_heuristic(current_plan, self.max_heuristic_iterations)?;
            log::debug!("Physical heuristic optimization completed successfully");
        }

        // Phase 5: Partitioning.
        current_plan = self.apply_partitioning_selection(current_plan, space, layout);

        Ok(current_plan)
    }

    fn ensure_logical_plan(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        if plan.logical_plan.is_none() {
            if let Some(root) = plan.root.clone() {
                let node_type = root.type_name().to_string();
                match crate::planning::plan::logical_plan::LogicalPlan::from_plan_node(&root) {
                    Ok(logical) => {
                        plan.set_logical_plan(logical);
                    }
                    Err(e) => {
                        let msg = format!(
                            "logical_plan fallback failed: {} (factorization skipped, flat execution)",
                            e
                        );
                        log::warn!("LogicalPlan::from_plan_node fallback failed: {}", e);
                        plan.cbo_notes.push(msg.clone());
                        plan.cbo_notes.push(format!(
                            "factorization: logical_plan_fallback_total=1 (node={})",
                            node_type
                        ));
                        if let Some(ref ms) = self.metrics_stats {
                            ms.add_value(graphdb_core::MetricType::FactorizationFallbackTotal);
                        }
                        if plan.parallel_fallback_reason.is_empty() {
                            plan.parallel_fallback_reason = msg;
                        } else {
                            plan.parallel_fallback_reason.push_str("; ");
                            plan.parallel_fallback_reason.push_str(&msg);
                        }
                    }
                }
            }
        }
        plan
    }

    fn apply_physical_mapping(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        // Logical → Physical mapping for factorization and index hints.
        //
        // The mapped tree is built from the logical plan in one pass, so
        // factorization operators keep their logical positions and hinted
        // scans become index scans. Merging with the physical root keeps
        // the cost-based choices that live only there (index scan limits,
        // TopN wiring). Structural divergences keep the physical subtree
        // and are recorded in `cbo_notes` instead of failing.
        if let Some(logical) = plan.logical_plan.clone() {
            if !crate::planning::physical_mapper::PhysicalMapper::needs_physical_mapping(
                &logical.root,
            ) {
                return plan;
            }
            if let Some(root) = plan.root.clone() {
                let mapped =
                    crate::planning::physical_mapper::PhysicalMapper::map(logical.root.clone());
                let (merged, notes) =
                    crate::planning::physical_mapper::PhysicalMapper::merge_physical_hints(
                        mapped, root,
                    );
                if notes.is_empty() {
                    log::debug!("PhysicalMapping: merged logical mapping into physical plan");
                } else {
                    let fallback_count = notes.len();
                    for note in &notes {
                        log::warn!("{note}");
                    }
                    plan.cbo_notes.extend(notes);
                    plan.cbo_notes.push(format!(
                        "factorization: physical_mapping_fallback_total={}",
                        fallback_count
                    ));
                    if let Some(ref ms) = self.metrics_stats {
                        ms.add_value_with_amount(
                            graphdb_core::MetricType::FactorizationPhysicalMappingFallbackTotal,
                            fallback_count as u64,
                        );
                    }
                }
                plan.set_root(merged);
            }
        }
        plan
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
            if plan.parallel_fallback_reason.is_empty() {
                plan.parallel_fallback_reason = decision.reason;
            } else {
                plan.parallel_fallback_reason.push_str("; ");
                plan.parallel_fallback_reason.push_str(&decision.reason);
            }
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

    /// Apply logical heuristic rules on the attached logical tree.
    fn apply_logical_heuristic(&self, mut plan: ExecutionPlan) -> OptimizeResult<ExecutionPlan> {
        self.logical_heuristic
            .set_max_iterations(self.max_heuristic_iterations);
        if let Some(mut logical) = plan.logical_plan.clone() {
            self.logical_heuristic.optimize(&mut logical.root)?;
            plan.set_logical_plan(logical);
        }
        Ok(plan)
    }

    /// Apply physical heuristic rules on the physical root.
    fn apply_physical_heuristic(
        &self,
        plan: ExecutionPlan,
        max_iterations: usize,
    ) -> OptimizeResult<ExecutionPlan> {
        // Interior mutability via AtomicUsize: set_max_iterations does not need &mut self.
        self.physical_heuristic.set_max_iterations(max_iterations);

        let root = match plan.root.clone() {
            Some(root) => root,
            None => return Ok(plan),
        };
        let result = self
            .physical_heuristic
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
    /// directly (`optimize_plan_nodes`). Structural rewrites are applied to
    /// the logical tree first so decision and rewrite share one fact source,
    /// and mirrored on the physical root, which is the artifact consumed by
    /// the physical planner.
    fn apply_cost_based(
        &self,
        plan: ExecutionPlan,
        space: Option<&str>,
    ) -> OptimizeResult<ExecutionPlan> {
        let mut plan = plan;
        let stats = StatsView::new(&self.stats_manager, space);

        let logical = plan.logical_plan().cloned();
        match logical {
            Some(_) => self.optimize_logical(&stats, space, &mut plan)?,
            None => self.optimize_plan_nodes(&stats, space, &mut plan)?,
        }
        Ok(plan)
    }

    /// Cost-based decisions driven by the logical plan tree.
    ///
    /// Decisions (join order, index selection, aggregate strategy) are taken
    /// on the pure logical tree attached during planning, and the structural
    /// rewrites are applied to the logical tree first so decision and rewrite
    /// share one fact source. The physical root receives the corresponding
    /// rewrite as well, because it is the artifact consumed by the physical
    /// planner; the post-mapping merge reconciles both sides.
    fn optimize_logical(
        &self,
        stats: &StatsView,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) -> OptimizeResult<()> {
        // Subquery unnesting (structural rewrite on the physical root).
        self.apply_unnesting(plan, stats);

        // Subquery unnesting on the logical tree (PatternApply → SemiJoin
        // for provably-safe simple shapes).
        self.apply_unnesting_logical(plan);

        // Join order — decision and rewrite on the logical tree, rewrite
        // mirrored on the physical root.
        self.apply_join_order_logical(stats, plan);

        // Cost-based index selection — decision stamped as hints on the
        // logical tree, structural rewrite on the physical root.
        self.apply_index_selection_logical(space, plan);

        // Sort + Limit → TopN conversion (residual patterns, cost-based).
        self.apply_topn_wiring(plan, stats);

        // Sort + Limit → TopN conversion on the logical tree.
        self.apply_topn_wiring_logical(stats, plan);

        // Aggregate strategy selection — decision on the logical
        // tree (the strategy is consumed by the physical planner via the
        // notes).
        self.apply_aggregate_strategy_logical(stats, plan);

        // Collect per-node row estimates for estimated_rows writeback.
        self.apply_row_estimates(plan, stats);

        // Expression precomputation decisions (note-only; EXPLAIN
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
        // Subquery unnesting (PatternApply → InnerJoin)
        self.apply_unnesting(plan, stats);

        // Join order optimization
        if let Some(ref root) = plan.root.clone() {
            let mut notes = Vec::new();
            let mut decisions = std::collections::HashMap::new();
            let rewritten = crate::optimizer::cost_based::join_order_rewriter::
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

        // Cost-based index selection (ScanVertices → IndexScan)
        if let Some(ref root) = plan.root.clone() {
            let selector = IndexSelector::new(
                self.cost_calculator.clone(),
                self.selectivity_estimator.clone(),
            );
            let mut notes = Vec::new();
            let rewritten = crate::optimizer::cost_based::index_selection::rewrite_index_scans(
                root,
                &selector,
                &self.stats_manager,
                space,
                &mut notes,
            );
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Sort + Limit → TopN conversion (residual patterns)
        self.apply_topn_wiring(plan, stats);

        // Aggregate strategy selection (decision notes)
        if let Some(ref root) = plan.root.clone() {
            let selector = AggregateStrategySelector::new(self.cost_calculator.clone());
            let mut notes = Vec::new();
            let rewritten = self.select_aggregate_strategies(root, stats, &selector, &mut notes);
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }

        // Collect per-node row estimates for estimated_rows writeback.
        self.apply_row_estimates(plan, stats);

        // Expression precomputation decisions (note-only)
        self.apply_precompute_notes(plan);

        Ok(())
    }

    /// Subquery unnesting: PatternApply → SemiJoin when cost-beneficial.
    fn apply_unnesting(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(ref root) = plan.root.clone() {
            let mut notes = Vec::new();
            let rewritten = self.unnest_subqueries(root, stats, &mut notes);
            plan.set_root(rewritten);
            plan.cbo_notes.extend(notes);
        }
    }

    /// Subquery unnesting on the logical tree (PatternApply → SemiJoin for
    /// provably-safe simple shapes).
    fn apply_unnesting_logical(&self, plan: &mut ExecutionPlan) {
        use crate::optimizer::cost_based::subquery_unnesting::unnest_pattern_applies_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };
        let mut notes = Vec::new();
        let rewritten = unnest_pattern_applies_logical(logical.root(), &mut notes);
        plan.cbo_notes.extend(notes);
        let mut updated_logical = logical;
        updated_logical.root = rewritten;
        plan.set_logical_plan(updated_logical);
    }

    /// Join order decision on the logical tree; structural rewrite applied
    /// to the physical root.
    fn apply_join_order_logical(&self, stats: &StatsView, plan: &mut ExecutionPlan) {
        use crate::optimizer::cost_based::join_order_rewriter::walk_and_optimize_joins_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };

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
        let mut updated_logical = logical;
        updated_logical.root = rewritten_logical;
        plan.set_logical_plan(updated_logical);

        // Structural rewrite on the physical root. The physical walker
        // recomputes the same decision; its notes are discarded here because
        // the logical walker is the note source.
        if let Some(ref root) = plan.root.clone() {
            let mut scratch = Vec::new();
            let mut decisions = std::collections::HashMap::new();
            let rewritten = crate::optimizer::cost_based::join_order_rewriter::
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
    fn apply_index_selection_logical(&self, space: Option<&str>, plan: &mut ExecutionPlan) {
        use crate::optimizer::cost_based::index_selection::rewrite_index_scans_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };

        let selector = IndexSelector::new(
            self.cost_calculator.clone(),
            self.selectivity_estimator.clone(),
        );

        // Decision on the logical tree, stamped as index hints.
        let mut notes = Vec::new();
        let rewritten_logical = rewrite_index_scans_logical(
            logical.root(),
            &selector,
            &self.stats_manager,
            space,
            &mut notes,
        );
        plan.cbo_notes.extend(notes);

        // Keep the attached logical plan in sync with the hint decision so
        // the full physical mapping rebuilds the chosen index access.
        let mut updated_logical = logical;
        updated_logical.root = rewritten_logical;
        plan.set_logical_plan(updated_logical);

        // Structural rewrite on the physical root (notes recomputed there
        // are discarded — the logical walker is the note source).
        if let Some(ref root) = plan.root.clone() {
            let mut scratch = Vec::new();
            let rewritten = crate::optimizer::cost_based::index_selection::rewrite_index_scans(
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
    fn apply_aggregate_strategy_logical(&self, stats: &StatsView, plan: &mut ExecutionPlan) {
        use crate::optimizer::cost_based::aggregate_strategy::walk_aggregate_strategies_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };
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

    /// Sort + Limit → TopN conversion on the logical tree (residual
    /// patterns, cost-based).
    fn apply_topn_wiring_logical(&self, stats: &StatsView, plan: &mut ExecutionPlan) {
        use crate::optimizer::cost_based::topn_wiring::rewrite_sort_with_limits_logical;

        let Some(logical) = plan.logical_plan().cloned() else {
            return;
        };
        let optimizer = SortEliminationOptimizer::new(self.cost_calculator.clone());
        let mut notes = Vec::new();
        let rewritten = rewrite_sort_with_limits_logical(
            logical.root(),
            &optimizer,
            stats,
            &self.selectivity_estimator,
            &mut notes,
        );
        plan.cbo_notes.extend(notes);
        let mut updated_logical = logical;
        updated_logical.root = rewritten;
        plan.set_logical_plan(updated_logical);
    }

    /// Sort + Limit → TopN conversion (residual patterns, cost-based).
    fn apply_topn_wiring(&self, plan: &mut ExecutionPlan, stats: &StatsView) {
        if let Some(ref root) = plan.root.clone() {
            let optimizer = SortEliminationOptimizer::new(self.cost_calculator.clone());
            let mut notes = Vec::new();
            let rewritten = crate::optimizer::cost_based::topn_wiring::rewrite_sort_with_limits(
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
                crate::optimizer::cost_based::row_estimates::collect_node_row_estimates(
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
            let optimizer = crate::optimizer::cost_based::expression_precomputation::ExpressionPrecomputationOptimizer::new(self.cost_calculator.clone());
            let notes =
                crate::optimizer::cost_based::precomputation_wiring::collect_precompute_notes(
                    root, &optimizer,
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
        use crate::optimizer::cost_based::row_estimates::estimate_node_output_rows;
        use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
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
        crate::optimizer::cost_based::traversal::rewrite_children(node, &mut closure)
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
        use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
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

    fn apply_remove_factorization(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        if let Some(logical) = plan.logical_plan.clone() {
            let mut root = logical.root.clone();
            crate::optimizer::factorization::RemoveFactorizationRewriter::new().rewrite(&mut root);
            let mut updated = logical.clone();
            updated.root = root;
            plan.set_logical_plan(updated);
            plan.cbo_notes
                .push("factorization: removed LogicalFlatten".to_string());
        }
        plan
    }

    fn apply_factorization(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        if let Some(logical) = plan.logical_plan.clone() {
            let mut root = logical.root.clone();
            crate::optimizer::factorization::FactorizationRewriter::new().rewrite(&mut root);
            let mut updated = logical.clone();
            updated.root = root.clone();
            plan.set_logical_plan(updated);
            let mut flattens = Vec::new();
            crate::planning::physical_mapper::PhysicalMapper::collect_flatten_positions(
                &root,
                &mut flattens,
            );
            flattens.sort_unstable();
            flattens.dedup();
            for pos in &flattens {
                plan.cbo_notes.push(format!("Flatten(group={})", pos));
            }
            // Record total flatten count for metrics observability.
            if !flattens.is_empty() {
                plan.cbo_notes
                    .push(format!("factorization: flatten_total={}", flattens.len()));
                if let Some(ref ms) = self.metrics_stats {
                    ms.add_value_with_amount(
                        graphdb_core::MetricType::FactorizationFlattenTotal,
                        flattens.len() as u64,
                    );
                }
            }
            // ExpandAll stays on the row path until its columnar batch path
            // is rebuilt on DataChunk (the removed heap row store is not
            // reused); record the retention so the degradation is visible
            // in EXPLAIN rather than silent.
            if Self::logical_contains_expand_all(&root) {
                plan.cbo_notes.push(
                    crate::executor::streaming::operators::graph_operator::expand::expand_all_row_path_note()
                        .to_string(),
                );
            }
            if plan
                .cbo_notes
                .iter()
                .all(|n| !n.starts_with("Flatten(group="))
            {
                plan.cbo_notes
                    .push("factorization: re-inserted LogicalFlatten".to_string());
            }
        }
        plan
    }

    /// Rewrite WCO intersect to hash join when the hash join is cheaper.
    ///
    /// Walks the logical tree to find `WcoIntersect` nodes, estimates both
    /// cost models, and replaces the node with a nested binary `InnerJoin`
    /// chain when the hash join wins. The replacement is validated against
    /// the factorized `at most one unflat group` invariant and reverted when
    /// the invariant is violated. Every rewrite and every retained WCO node
    /// is recorded in `cbo_notes`.
    fn apply_intersect_to_join_rewrite(
        &self,
        mut plan: ExecutionPlan,
        space: Option<&str>,
    ) -> ExecutionPlan {
        let stats = StatsView::new(&self.stats_manager, space);
        if let Some(ref logical) = plan.logical_plan.clone() {
            let mut root = logical.root.clone();
            let mut notes = Vec::new();
            let mut rewrite_count = 0u64;
            Self::rewrite_intersect_to_join(
                &mut root,
                &stats,
                &self.selectivity_estimator,
                &mut notes,
                &mut rewrite_count,
            );
            if rewrite_count > 0 {
                if Self::validate_factorized_invariant(&root) {
                    let mut updated = logical.clone();
                    updated.root = root;
                    plan.set_logical_plan(updated);
                    plan.cbo_notes.extend(notes);
                    plan.cbo_notes.push(format!(
                        "factorization: intersect_to_join_rewrite_total={}",
                        rewrite_count
                    ));
                    if let Some(ref ms) = self.metrics_stats {
                        ms.add_value_with_amount(
                            graphdb_core::MetricType::FactorizationFallbackTotal,
                            rewrite_count,
                        );
                    }
                } else {
                    plan.cbo_notes.extend(notes);
                    plan.cbo_notes.push(
                        "factorization: intersect_to_join_rewrite_total=0 \
                         (reverted, schema invariant violated)"
                            .to_string(),
                    );
                }
            } else if !notes.is_empty() {
                plan.cbo_notes.extend(notes);
            }
        }
        plan
    }

    /// Maximum build sides rewritten from one `WcoIntersect` into a nested
    /// binary join chain. Wider intersects keep the WCO operator so the plan
    /// depth stays bounded.
    const MAX_INTERSECT_REWRITE_BUILDS: usize = 8;

    fn rewrite_intersect_to_join(
        node: &mut LogicalNodeEnum,
        stats: &StatsView,
        selectivity: &crate::optimizer::SelectivityEstimator,
        notes: &mut Vec<String>,
        rewrite_count: &mut u64,
    ) {
        for child in Self::logical_children_mut(node) {
            Self::rewrite_intersect_to_join(child, stats, selectivity, notes, rewrite_count);
        }

        let snapshot = node.clone();
        let LogicalNodeEnum::WcoIntersect(wco) = &snapshot else {
            return;
        };
        use crate::optimizer::cost_based::row_estimates::estimate_node_output_rows_logical;
        use crate::planning::join_order::cost_model::CostModel;

        let probe_rows = estimate_node_output_rows_logical(wco.probe_side(), stats, selectivity);
        let mut build_costs = Vec::with_capacity(wco.num_builds());
        let mut total_build_cost = 0u64;
        for build in wco.build_sides() {
            let rows = estimate_node_output_rows_logical(build, stats, selectivity);
            total_build_cost = total_build_cost.saturating_add(rows);
            build_costs.push(rows);
        }
        let output_rows = probe_rows.min(total_build_cost).max(1);
        let intersect_cost =
            CostModel::compute_intersect_cost(0, probe_rows, &build_costs, output_rows);
        let join_key_cardinality = probe_rows;
        let hash_cost = CostModel::compute_hash_join_cost(
            0,
            probe_rows,
            total_build_cost,
            join_key_cardinality,
        );
        if hash_cost >= intersect_cost {
            return;
        }
        if wco.num_builds() > Self::MAX_INTERSECT_REWRITE_BUILDS {
            notes.push(format!(
                "factorization: WcoIntersect kept (intersect_cost={}, hash_cost={}, \
                 reason: {} build sides exceed rewrite limit {})",
                intersect_cost,
                hash_cost,
                wco.num_builds(),
                Self::MAX_INTERSECT_REWRITE_BUILDS,
            ));
            return;
        }
        *node = Self::build_join_chain_from_intersect(wco);
        *rewrite_count += 1;
        notes.push(format!(
            "factorization: WcoIntersect fallback to HashJoin \
             (intersect_cost={}, hash_cost={}, reason: hash join cheaper)",
            intersect_cost, hash_cost,
        ));
    }

    /// Build a left-deep nested binary `InnerJoin` chain from one N-way
    /// `WcoIntersect`: the probe side becomes the leftmost input and every
    /// build side folds in on the shared intersect key.
    fn build_join_chain_from_intersect(
        wco: &crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode,
    ) -> LogicalNodeEnum {
        use crate::planning::plan::core::node_id_generator::next_node_id;
        use crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode;

        let intersect_key = wco.intersect_key().clone();
        let mut acc = wco.probe_side().clone();
        for build in wco.build_sides() {
            let mut col_names = acc.col_names().to_vec();
            if let Some(name) = intersect_key.as_variable() {
                if !col_names.iter().any(|c| c == &name) {
                    col_names.push(name);
                }
            }
            for col in build.col_names() {
                if !col_names.contains(col) {
                    col_names.push(col.clone());
                }
            }
            let left = acc.clone();
            let right = build.clone();
            let join = LogicalInnerJoinNode {
                id: next_node_id(),
                left: Box::new(left.clone()),
                right: Box::new(right.clone()),
                hash_keys: vec![intersect_key.clone()],
                probe_keys: vec![intersect_key.clone()],
                deps: vec![left, right],
                recommended_algorithm: None,
                output_var: wco.output_var().map(|s| s.to_string()),
                col_names,
                column_types: vec![],
            };
            acc = LogicalNodeEnum::InnerJoin(join);
        }
        acc
    }

    /// Check the factorized `at most one unflat group` invariant over the
    /// whole logical tree by bottom-up schema computation.
    fn validate_factorized_invariant(root: &LogicalNodeEnum) -> bool {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::compute_schema_tree(root);
        }))
        .is_ok()
    }

    fn compute_schema_tree(
        node: &LogicalNodeEnum,
    ) -> crate::planning::plan::factorization::FactorizedSchema {
        use crate::planning::plan::factorization::FactorizedSchemaCompute;
        let child_schemas: Vec<_> = crate::planning::physical_mapper::logical_children(node)
            .iter()
            .map(|child| Self::compute_schema_tree(child))
            .collect();
        let mut owned = node.clone();
        owned.compute_factorized_schema(&child_schemas)
    }

    /// Mutable children of a logical node for in-place rewrites.
    fn logical_children_mut(node: &mut LogicalNodeEnum) -> Vec<&mut LogicalNodeEnum> {
        match node {
            LogicalNodeEnum::Flatten(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Project(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Filter(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Sort(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Limit(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Skip(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::TopN(n) => n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Sample(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Dedup(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Aggregate(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Window(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Traverse(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Unwind(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Remove(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::PipeDeleteVertices(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::PipeDeleteEdges(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::DataCollect(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Materialize(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::RollUpApply(n) => {
                n.input.as_deref_mut().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Assign(n) => {
                let mut out = Vec::new();
                if let Some(c) = n.input.as_deref_mut() {
                    out.push(c);
                }
                out.extend(n.deps.iter_mut());
                out
            }
            LogicalNodeEnum::Select(n) => {
                let mut out = Vec::new();
                if let Some(b) = n.if_branch.as_deref_mut() {
                    out.push(b);
                }
                if let Some(b) = n.else_branch.as_deref_mut() {
                    out.push(b);
                }
                out
            }
            LogicalNodeEnum::Loop(n) => n.body.as_deref_mut().map(|b| vec![b]).unwrap_or_default(),
            LogicalNodeEnum::InnerJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::LeftJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::RightJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::CrossJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::FullOuterJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::SemiJoin(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::PatternApply(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::CorrelatedApply(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::Apply(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::BiExpand(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::BiTraverse(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::MultiShortestPath(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::BFSShortest(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::AllPaths(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::ShortestPath(n) => vec![n.left.as_mut(), n.right.as_mut()],
            LogicalNodeEnum::Expand(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::ExpandAll(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::AppendVertices(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::GetVertices(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::GetNeighbors(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::Union(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::Minus(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::Intersect(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::WcoIntersect(n) => n.deps.iter_mut().collect(),
            LogicalNodeEnum::Start(_)
            | LogicalNodeEnum::ScanVertices(_)
            | LogicalNodeEnum::ScanEdges(_)
            | LogicalNodeEnum::GetEdges(_)
            | LogicalNodeEnum::Argument(_)
            | LogicalNodeEnum::PassThrough(_)
            | LogicalNodeEnum::BeginTransaction(_)
            | LogicalNodeEnum::Commit(_)
            | LogicalNodeEnum::Rollback(_)
            | LogicalNodeEnum::InsertVertices(_)
            | LogicalNodeEnum::InsertEdges(_)
            | LogicalNodeEnum::Update(_)
            | LogicalNodeEnum::DeleteVertices(_)
            | LogicalNodeEnum::DeleteEdges(_)
            | LogicalNodeEnum::DeleteTags(_)
            | LogicalNodeEnum::DeleteIndex(_)
            | LogicalNodeEnum::CopyFrom(_)
            | LogicalNodeEnum::CopyTo(_)
            | LogicalNodeEnum::FulltextSearch(_)
            | LogicalNodeEnum::FulltextLookup(_)
            | LogicalNodeEnum::MatchFulltext(_) => vec![],
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorSearch(_)
            | LogicalNodeEnum::VectorLookup(_)
            | LogicalNodeEnum::VectorMatch(_) => vec![],
        }
    }

    /// Whether the logical tree contains an `ExpandAll` node.
    ///
    /// Used to record the row-path retention note for `ExpandAll` in
    /// `cbo_notes` (see `expand_all_row_path_note`).
    fn logical_contains_expand_all(node: &crate::planning::plan::logical::LogicalNodeEnum) -> bool {
        if matches!(
            node,
            crate::planning::plan::logical::LogicalNodeEnum::ExpandAll(_)
        ) {
            return true;
        }
        crate::planning::physical_mapper::logical_children(node)
            .iter()
            .any(|child| Self::logical_contains_expand_all(child))
    }

    /// Get the physical heuristic batch optimizer
    pub fn heuristic_batch(&self) -> &BatchOptimizer {
        self.physical_heuristic.batch()
    }

    /// Get the logical heuristic optimizer
    pub fn logical_heuristic(&self) -> &LogicalBatchOptimizer {
        &self.logical_heuristic
    }

    /// Get the physical heuristic optimizer
    pub fn physical_heuristic(&self) -> &PhysicalHeuristicOptimizer {
        &self.physical_heuristic
    }

    /// Get the batch statistics of the last heuristic optimization (for EXPLAIN diagnostics)
    pub fn last_batch_statistics(&self) -> Vec<(OptimizationBatch, BatchStatistics)> {
        match self.last_batch_statistics.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

use crate::optimizer::error::{OptimizeError, OptimizeResult};

impl Default for OptimizerEngine {
    fn default() -> Self {
        Self::new(CostModelConfig::default())
    }
}
