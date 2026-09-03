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

use crate::planning::plan::logical_plan::LogicalPlan;
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
            columnar_policy: Arc::new(crate::executor::streaming::chunk::ColumnarPolicy::default()),
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
        // Legacy paths that still emit physical trees are bridged via the
        // reverse conversion instead of being silently skipped.
        current_plan = self.ensure_logical_plan(current_plan);

        // Factorization: remove factorization before heuristic passes so they
        // operate on a flat view. Mirrors `RemoveFactorizationRewriter` at
        // `optimizer.cpp:1`.
        current_plan = self.apply_remove_factorization(current_plan);

        // Logical heuristic optimization on the logical tree.
        if self.enable_heuristic {
            log::debug!("Starting logical heuristic optimization");
            current_plan = self
                .apply_heuristic_with_max_iterations(current_plan, self.max_heuristic_iterations)?;
            log::debug!("Logical heuristic optimization completed successfully");
        }

        // Cost-based optimization (always active — conservative rules)
        log::debug!("Starting cost-based optimization");
        current_plan = self.apply_cost_based(current_plan, space)?;
        log::debug!("Cost-based optimization completed successfully");

        // Factorization: re-insert flatten operators after all optimizations.
        // Mirrors `FactorizationRewriter` at `optimizer.cpp:4`.
        current_plan = self.apply_factorization(current_plan);

        // Physical mapping: LogicalNodeEnum → PlanNodeEnum (introduces
        // physical choices such as IndexScan). Mirrors PhysicalMapper.
        // This is a splice implementation to protect CBO's IndexScan choice
        // (see physical_planner.rs:34 which would discard CBO rewrites if using
        // full convert_logical_to_physical). Once CBO marks index_hint on
        // Logical, switch to full PhysicalMapper::map.
        current_plan = self.apply_physical_mapping(current_plan);

        // Physical heuristic optimization on the physical tree.
        if self.enable_heuristic {
            log::debug!("Starting physical heuristic optimization");
            current_plan = self
                .apply_heuristic_with_max_iterations(current_plan, self.max_heuristic_iterations)?;
            log::debug!("Physical heuristic optimization completed successfully");
        }

        current_plan = self.apply_partitioning_selection(current_plan, space, layout);

        Ok(current_plan)
    }

    fn ensure_logical_plan(&self, mut plan: ExecutionPlan) -> ExecutionPlan {
        if plan.logical_plan.is_none() {
            if let Some(root) = plan.root.clone() {
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
        // Logical → Physical mapping for factorization.
        //
        // The generic Logical→Physical converter would discard CBO rewrites
        // that currently operate directly on the physical root (IndexScan,
        // TopN). To avoid that regression we do not overwrite the physical
        // root wholesale. Instead we splice the LogicalFlatten nodes inserted
        // by FactorizationRewriter into the physical tree at the corresponding
        // positions, preserving all other physical choices.
        //
        // If the logical plan contains Flatten but the physical does not,
        // failing to splice would silently drop factorization (wrong row
        // counts). That fallback is semantically harmful and must be observable.
        if let Some(logical) = plan.logical_plan.clone() {
            let has_logical_flatten =
                crate::optimizer::factorization::RemoveFactorizationRewriter::has_flatten_public(
                    &logical.root,
                );
            let has_physical_flatten = plan
                .root
                .as_ref()
                .map(|r| Self::physical_has_flatten(r))
                .unwrap_or(false);
            if has_logical_flatten && !has_physical_flatten {
                if let Some(root) = plan.root.clone() {
                    match Self::splice_flatten_from_logical(&logical.root, &root) {
                        Ok(new_root) => {
                            plan.set_root(new_root);
                            log::debug!(
                                "PhysicalMapping: spliced LogicalFlatten into physical plan"
                            );
                        }
                        Err(e) => {
                            let msg = format!("PhysicalMapping: failed to splice Flatten ({}); factorization will be ineffective", e);
                            log::warn!("{}", msg);
                            plan.cbo_notes.push(msg.clone());
                            if plan.parallel_fallback_reason.is_empty() {
                                plan.parallel_fallback_reason = msg;
                            } else {
                                plan.parallel_fallback_reason.push_str("; ");
                                plan.parallel_fallback_reason.push_str(&msg);
                            }
                        }
                    }
                }
            } else if has_logical_flatten {
                log::debug!("PhysicalMapping: physical already contains Flatten, no splice needed");
            }
        }
        plan
    }

    fn physical_has_flatten(node: &crate::planning::plan::PlanNodeEnum) -> bool {
        if matches!(node, crate::planning::plan::PlanNodeEnum::Flatten(_)) {
            return true;
        }
        for child in node.children() {
            if Self::physical_has_flatten(child) {
                return true;
            }
        }
        false
    }

    fn splice_flatten_from_logical(
        logical: &crate::planning::plan::logical::LogicalNodeEnum,
        physical: &crate::planning::plan::PlanNodeEnum,
    ) -> Result<crate::planning::plan::PlanNodeEnum, String> {
        Self::splice_logical_physical(logical, physical)
    }

    fn splice_logical_physical(
        logical: &crate::planning::plan::logical::LogicalNodeEnum,
        physical: &crate::planning::plan::PlanNodeEnum,
    ) -> Result<crate::planning::plan::PlanNodeEnum, String> {
        use crate::planning::plan::logical::LogicalNodeEnum;
        if let LogicalNodeEnum::Flatten(fl) = logical {
            let child = fl
                .input
                .as_ref()
                .ok_or_else(|| "LogicalFlatten missing input".to_string())?;
            let spliced = Self::splice_logical_physical(child, physical)?;
            let flatten_node =
                crate::planning::plan::core::nodes::operation::flatten_node::FlattenNode::new(
                    spliced,
                    fl.group_pos,
                )
                .map_err(|e| e.to_string())?;
            return Ok(crate::planning::plan::PlanNodeEnum::Flatten(flatten_node));
        }
        let log_children = Self::logical_children(logical);
        let phys_children = physical.children();
        if log_children.len() != phys_children.len() {
            let mut flattens = Vec::new();
            Self::collect_logical_flattens(logical, &mut flattens);
            flattens.sort_unstable();
            flattens.dedup();
            if flattens.is_empty() {
                return Ok(physical.clone());
            }
            return Err(format!(
                "splice LogicalFlatten: structure mismatch logical {} vs physical {} (logical children {}, physical children {}), LogicalFlatten positions {:?} cannot be spliced; CBO rewrites diverged physical tree",
                logical.type_name(),
                physical.type_name(),
                log_children.len(),
                phys_children.len(),
                flattens
            ));
        }
        if log_children.is_empty() {
            return Ok(physical.clone());
        }
        let mut new_children = Vec::new();
        for (lc, pc) in log_children.iter().zip(phys_children.iter()) {
            new_children.push(Self::splice_logical_physical(lc, pc)?);
        }
        Self::rebuild_physical_with_new_children(physical, new_children)
    }

    fn logical_children<'a>(
        node: &'a crate::planning::plan::logical::LogicalNodeEnum,
    ) -> Vec<&'a crate::planning::plan::logical::LogicalNodeEnum> {
        use crate::planning::plan::logical::LogicalNodeEnum;
        match node {
            LogicalNodeEnum::Flatten(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Project(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Filter(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Sort(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Limit(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::TopN(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Sample(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Dedup(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Aggregate(n) => {
                n.input.as_deref().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Window(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Traverse(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Assign(n) => {
                let mut v = Vec::new();
                if let Some(c) = n.input.as_deref() {
                    v.push(c);
                }
                for d in &n.deps {
                    v.push(d);
                }
                v
            }
            LogicalNodeEnum::Remove(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::DataCollect(n) => {
                n.input.as_deref().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Materialize(n) => {
                n.input.as_deref().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::RollUpApply(n) => {
                n.input.as_deref().map(|c| vec![c]).unwrap_or_default()
            }
            LogicalNodeEnum::Unwind(n) => n.input.as_deref().map(|c| vec![c]).unwrap_or_default(),
            LogicalNodeEnum::Select(n) => {
                let mut v = Vec::new();
                if let Some(b) = n.if_branch() {
                    v.push(b);
                }
                if let Some(b) = n.else_branch() {
                    v.push(b);
                }
                v
            }
            LogicalNodeEnum::Loop(n) => n.body().map(|b| vec![b]).unwrap_or_default(),
            LogicalNodeEnum::InnerJoin(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::LeftJoin(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::RightJoin(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::CrossJoin(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::FullOuterJoin(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::SemiJoin(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::PatternApply(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::CorrelatedApply(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::Apply(n) => vec![n.left_input(), n.right_input()],
            LogicalNodeEnum::BiExpand(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::BiTraverse(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::MultiShortestPath(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::BFSShortest(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::AllPaths(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::ShortestPath(n) => vec![&n.left, &n.right],
            LogicalNodeEnum::Expand(n) => n.deps.iter().collect(),
            LogicalNodeEnum::ExpandAll(n) => n.deps.iter().collect(),
            LogicalNodeEnum::AppendVertices(n) => n.deps.iter().collect(),
            LogicalNodeEnum::GetVertices(n) => n.deps.iter().collect(),
            LogicalNodeEnum::GetNeighbors(n) => n.deps.iter().collect(),
            LogicalNodeEnum::Union(n) => n.deps.iter().collect(),
            LogicalNodeEnum::Minus(n) => n.deps.iter().collect(),
            LogicalNodeEnum::Intersect(n) => n.deps.iter().collect(),
            LogicalNodeEnum::Start(_)
            | LogicalNodeEnum::ScanVertices(_)
            | LogicalNodeEnum::ScanEdges(_)
            | LogicalNodeEnum::GetEdges(_)
            | LogicalNodeEnum::Argument(_)
            | LogicalNodeEnum::PassThrough(_)
            | LogicalNodeEnum::BeginTransaction(_)
            | LogicalNodeEnum::Commit(_)
            | LogicalNodeEnum::Rollback(_)
            | LogicalNodeEnum::FulltextSearch(_)
            | LogicalNodeEnum::FulltextLookup(_)
            | LogicalNodeEnum::MatchFulltext(_) => vec![],
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorSearch(_)
            | LogicalNodeEnum::VectorLookup(_)
            | LogicalNodeEnum::VectorMatch(_) => vec![],
        }
    }

    fn rebuild_physical_with_new_children(
        physical: &crate::planning::plan::PlanNodeEnum,
        new_children: Vec<crate::planning::plan::PlanNodeEnum>,
    ) -> Result<crate::planning::plan::PlanNodeEnum, String> {
        use crate::planning::plan::core::nodes::base::plan_node_traits::{
            BinaryInputNode, MultipleInputNode, SingleInputNode,
        };
        use crate::planning::plan::PlanNodeEnum;
        match physical {
            PlanNodeEnum::Project(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Project")?,
                );
                Ok(PlanNodeEnum::Project(cloned))
            }
            PlanNodeEnum::Filter(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Filter")?,
                );
                Ok(PlanNodeEnum::Filter(cloned))
            }
            PlanNodeEnum::Sort(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Sort")?,
                );
                Ok(PlanNodeEnum::Sort(cloned))
            }
            PlanNodeEnum::Limit(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Limit")?,
                );
                Ok(PlanNodeEnum::Limit(cloned))
            }
            PlanNodeEnum::TopN(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for TopN")?,
                );
                Ok(PlanNodeEnum::TopN(cloned))
            }
            PlanNodeEnum::Sample(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Sample")?,
                );
                Ok(PlanNodeEnum::Sample(cloned))
            }
            PlanNodeEnum::Dedup(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Dedup")?,
                );
                Ok(PlanNodeEnum::Dedup(cloned))
            }
            PlanNodeEnum::Aggregate(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Aggregate")?,
                );
                Ok(PlanNodeEnum::Aggregate(cloned))
            }
            PlanNodeEnum::Window(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Window")?,
                );
                Ok(PlanNodeEnum::Window(cloned))
            }
            PlanNodeEnum::Traverse(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Traverse")?,
                );
                Ok(PlanNodeEnum::Traverse(cloned))
            }
            PlanNodeEnum::Unwind(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Unwind")?,
                );
                Ok(PlanNodeEnum::Unwind(cloned))
            }
            PlanNodeEnum::Assign(n) => {
                let mut cloned = n.clone();
                if new_children.is_empty() {
                    return Err("missing children for Assign".to_string());
                }
                cloned.set_input(new_children[0].clone());
                if new_children.len() > 1 {
                    let mut deps = cloned.dependencies().to_vec();
                    for (i, c) in new_children.iter().skip(1).enumerate() {
                        if i < deps.len() {
                            deps[i] = c.clone();
                        }
                    }
                    cloned.set_dependencies(deps);
                }
                Ok(PlanNodeEnum::Assign(cloned))
            }
            PlanNodeEnum::DataCollect(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for DataCollect")?,
                );
                Ok(PlanNodeEnum::DataCollect(cloned))
            }
            PlanNodeEnum::Remove(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Remove")?,
                );
                Ok(PlanNodeEnum::Remove(cloned))
            }
            PlanNodeEnum::Materialize(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for Materialize")?,
                );
                Ok(PlanNodeEnum::Materialize(cloned))
            }
            PlanNodeEnum::RollUpApply(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for RollUpApply")?,
                );
                Ok(PlanNodeEnum::RollUpApply(cloned))
            }
            PlanNodeEnum::PatternApply(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for PatternApply")?,
                );
                Ok(PlanNodeEnum::PatternApply(cloned))
            }
            PlanNodeEnum::CorrelatedApply(n) => {
                let mut cloned = n.clone();
                cloned.set_input(
                    new_children
                        .into_iter()
                        .next()
                        .ok_or("missing child for CorrelatedApply")?,
                );
                Ok(PlanNodeEnum::CorrelatedApply(cloned))
            }
            PlanNodeEnum::InnerJoin(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("InnerJoin requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::InnerJoin(cloned))
            }
            PlanNodeEnum::LeftJoin(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("LeftJoin requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::LeftJoin(cloned))
            }
            PlanNodeEnum::RightJoin(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("RightJoin requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::RightJoin(cloned))
            }
            PlanNodeEnum::CrossJoin(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("CrossJoin requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::CrossJoin(cloned))
            }
            PlanNodeEnum::FullOuterJoin(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("FullOuterJoin requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::FullOuterJoin(cloned))
            }
            PlanNodeEnum::SemiJoin(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("SemiJoin requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::SemiJoin(cloned))
            }
            PlanNodeEnum::Apply(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("Apply requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::Apply(cloned))
            }
            PlanNodeEnum::BiExpand(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("BiExpand requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::BiExpand(cloned))
            }
            PlanNodeEnum::BiTraverse(n) => {
                let mut cloned = n.clone();
                if new_children.len() != 2 {
                    return Err("BiTraverse requires 2 children".to_string());
                }
                cloned.set_left_input(new_children[0].clone());
                cloned.set_right_input(new_children[1].clone());
                Ok(PlanNodeEnum::BiTraverse(cloned))
            }
            PlanNodeEnum::Expand(n) => {
                let mut cloned = n.clone();
                *cloned.inputs_mut() = new_children;
                Ok(PlanNodeEnum::Expand(cloned))
            }
            PlanNodeEnum::ExpandAll(n) => {
                let mut cloned = n.clone();
                *cloned.inputs_mut() = new_children;
                Ok(PlanNodeEnum::ExpandAll(cloned))
            }
            PlanNodeEnum::AppendVertices(n) => {
                let mut cloned = n.clone();
                *cloned.inputs_mut() = new_children;
                Ok(PlanNodeEnum::AppendVertices(cloned))
            }
            PlanNodeEnum::Union(n) => {
                let mut cloned = n.clone();
                *cloned.dependencies_mut() = new_children.clone();
                if let Some(first) = new_children.first() {
                    *cloned.input_mut() = first.clone();
                }
                Ok(PlanNodeEnum::Union(cloned))
            }
            PlanNodeEnum::Minus(n) => {
                let mut cloned = n.clone();
                *cloned.dependencies_mut() = new_children.clone();
                if let Some(first) = new_children.first() {
                    *cloned.input_mut() = first.clone();
                }
                Ok(PlanNodeEnum::Minus(cloned))
            }
            PlanNodeEnum::Intersect(n) => {
                let mut cloned = n.clone();
                *cloned.dependencies_mut() = new_children.clone();
                if let Some(first) = new_children.first() {
                    *cloned.input_mut() = first.clone();
                }
                Ok(PlanNodeEnum::Intersect(cloned))
            }
            PlanNodeEnum::Select(n) => {
                let mut cloned = n.clone();
                let orig_has_if = n.if_branch().is_some();
                let orig_has_else = n.else_branch().is_some();
                let mut idx = 0;
                if orig_has_if {
                    if idx < new_children.len() {
                        cloned.set_if_branch(new_children[idx].clone());
                        idx += 1;
                    }
                }
                if orig_has_else {
                    if idx < new_children.len() {
                        cloned.set_else_branch(new_children[idx].clone());
                    }
                }
                Ok(PlanNodeEnum::Select(cloned))
            }
            PlanNodeEnum::Loop(n) => {
                let mut cloned = n.clone();
                if let Some(new_body) = new_children.into_iter().next() {
                    cloned.set_body(new_body);
                }
                Ok(PlanNodeEnum::Loop(cloned))
            }
            _ => Ok(physical.clone()),
        }
    }

    fn collect_logical_flattens(
        node: &crate::planning::plan::logical::LogicalNodeEnum,
        out: &mut Vec<u32>,
    ) {
        use crate::planning::plan::logical::LogicalNodeEnum;
        match node {
            LogicalNodeEnum::Flatten(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
                out.push(n.group_pos);
            }
            LogicalNodeEnum::Project(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Filter(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Sort(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Limit(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::TopN(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Dedup(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Aggregate(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Window(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Sample(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Unwind(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Traverse(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Assign(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::Remove(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::DataCollect(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::Materialize(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::RollUpApply(n) => {
                if let Some(child) = &n.input {
                    Self::collect_logical_flattens(child, out);
                }
            }
            LogicalNodeEnum::GetVertices(n) => {
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::GetNeighbors(n) => {
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::Expand(n) => {
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::ExpandAll(n) => {
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::AppendVertices(n) => {
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::BiExpand(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::BiTraverse(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::InnerJoin(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::LeftJoin(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::RightJoin(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::CrossJoin(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::FullOuterJoin(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::SemiJoin(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::PatternApply(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::CorrelatedApply(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::Apply(n) => {
                Self::collect_logical_flattens(n.left_input(), out);
                Self::collect_logical_flattens(n.right_input(), out);
            }
            LogicalNodeEnum::Union(n) => {
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::Minus(n) => {
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::Intersect(n) => {
                for d in &n.deps {
                    Self::collect_logical_flattens(d, out);
                }
            }
            LogicalNodeEnum::Select(n) => {
                if let Some(b) = n.if_branch() {
                    Self::collect_logical_flattens(b, out);
                }
                if let Some(b) = n.else_branch() {
                    Self::collect_logical_flattens(b, out);
                }
            }
            LogicalNodeEnum::Loop(n) => {
                if let Some(b) = n.body() {
                    Self::collect_logical_flattens(b, out);
                }
            }
            LogicalNodeEnum::MultiShortestPath(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::BFSShortest(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::AllPaths(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            LogicalNodeEnum::ShortestPath(n) => {
                Self::collect_logical_flattens(&n.left, out);
                Self::collect_logical_flattens(&n.right, out);
            }
            _ => {}
        }
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
        // Subquery unnesting (structural rewrite on the physical
        // root; the logical tree cannot represent PatternApply, so plans
        // containing one fall back to `optimize_plan_nodes` — defensive).
        self.apply_unnesting(plan, stats);

        // Join order — decision on the logical tree, rewrite on
        // the physical root.
        self.apply_join_order_logical(logical, stats, plan);

        // Cost-based index selection — decision on the logical
        // tree, rewrite on the physical root.
        self.apply_index_selection_logical(logical, space, plan);

        // Sort + Limit → TopN conversion (residual patterns,
        // cost-based; physical structural rewrite).
        self.apply_topn_wiring(plan, stats);

        // Aggregate strategy selection — decision on the logical
        // tree (the strategy is consumed by the physical planner via the
        // notes).
        self.apply_aggregate_strategy_logical(logical, stats, plan);

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
        use crate::optimizer::cost_based::join_order_rewriter::walk_and_optimize_joins_logical;

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
    fn apply_index_selection_logical(
        &self,
        logical: &LogicalPlan,
        space: Option<&str>,
        plan: &mut ExecutionPlan,
    ) {
        use crate::optimizer::cost_based::index_selection::rewrite_index_scans_logical;

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
    fn apply_aggregate_strategy_logical(
        &self,
        logical: &LogicalPlan,
        stats: &StatsView,
        plan: &mut ExecutionPlan,
    ) {
        use crate::optimizer::cost_based::aggregate_strategy::walk_aggregate_strategies_logical;

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
            Self::collect_logical_flattens(&root, &mut flattens);
            flattens.sort_unstable();
            flattens.dedup();
            for pos in flattens {
                plan.cbo_notes.push(format!("Flatten(group={})", pos));
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

use crate::optimizer::error::{OptimizeError, OptimizeResult};

impl Default for OptimizerEngine {
    fn default() -> Self {
        Self::new(CostModelConfig::default())
    }
}
