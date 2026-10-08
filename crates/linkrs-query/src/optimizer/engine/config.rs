use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::optimizer::heuristic::{LogicalBatchOptimizer, PhysicalHeuristicOptimizer};
use crate::optimizer::partitioning::{PartitioningConfig, PartitioningPlanner};
use crate::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
use crate::optimizer::stats::feedback::decision::DecisionFeedbackStore;
use crate::optimizer::stats::feedback::history::QueryFeedbackHistory;
use crate::optimizer::stats::feedback::selectivity::SelectivityFeedbackManager;
use crate::optimizer::stats::feedback::trigger::AutoFeedbackTrigger;
use crate::optimizer::{
    BatchPlanAnalyzer, CostCalculator, CostModelConfig, CteCacheManager, SelectivityEstimator,
    StatisticsManager, SubqueryUnnestingOptimizer,
};
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;

use super::OptimizerEngine;

impl OptimizerEngine {
    pub fn new(cost_config: CostModelConfig) -> Self {
        Self::with_expression_context(Arc::new(ExpressionAnalysisContext::new()), cost_config)
    }

    pub fn with_expression_context(
        expression_context: Arc<ExpressionAnalysisContext>,
        cost_config: CostModelConfig,
    ) -> Self {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cte_cache_manager = Arc::new(CteCacheManager::new());

        Self::with_components(
            expression_context,
            stats_manager,
            cte_cache_manager,
            cost_config,
            None,
        )
    }

    pub(crate) fn with_components(
        expression_context: Arc<ExpressionAnalysisContext>,
        stats_manager: Arc<StatisticsManager>,
        cte_cache_manager: Arc<CteCacheManager>,
        cost_config: CostModelConfig,
        metrics_stats: Option<Arc<linkrs_metrics::StatsManager>>,
    ) -> Self {
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

        let batch_plan_analyzer = BatchPlanAnalyzer::new();
        let subquery_unnesting_optimizer = SubqueryUnnestingOptimizer::new();

        let logical_heuristic = LogicalBatchOptimizer::new();
        let physical_heuristic = PhysicalHeuristicOptimizer::from_registry(
            crate::optimizer::heuristic::rule_enum::RuleRegistry::default(),
        );

        Self {
            expression_context,
            stats_manager,
            cte_cache_manager,
            cost_calculator,
            selectivity_estimator,
            batch_plan_analyzer,
            subquery_unnesting_optimizer,
            cost_config,
            space_cost_configs: parking_lot::RwLock::new(HashMap::new()),
            space_calculators: parking_lot::RwLock::new(HashMap::new()),
            cost_epoch: AtomicU64::new(0),
            logical_heuristic,
            physical_heuristic,
            last_batch_statistics: std::sync::Mutex::new(Vec::new()),
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

    pub fn for_ssd() -> Self {
        Self::new(CostModelConfig::for_ssd())
    }

    pub fn for_in_memory() -> Self {
        Self::new(CostModelConfig::for_in_memory())
    }

    pub fn cost_config(&self) -> &CostModelConfig {
        &self.cost_config
    }

    pub fn cost_calculator(&self) -> &Arc<CostCalculator> {
        &self.cost_calculator
    }

    pub fn cost_calculator_for(&self, space: Option<&str>) -> Arc<CostCalculator> {
        let Some(space) = space else {
            return self.cost_calculator.clone();
        };
        if let Some(cached) = self.space_calculators.read().get(space) {
            return cached.clone();
        }
        let config = self
            .space_cost_configs
            .read()
            .get(space)
            .copied()
            .unwrap_or(self.cost_config);
        let calculator = Arc::new(CostCalculator::with_config(
            self.stats_manager.clone(),
            config,
        ));
        self.space_calculators
            .write()
            .insert(space.to_string(), calculator.clone());
        calculator
    }

    pub fn space_cost_configs(&self) -> HashMap<String, CostModelConfig> {
        self.space_cost_configs.read().clone()
    }

    pub fn set_space_cost_configs(&self, configs: HashMap<String, CostModelConfig>) {
        *self.space_cost_configs.write() = configs;
        self.space_calculators.write().clear();
        self.cost_epoch.fetch_add(1, Ordering::Relaxed);
    }

    pub fn cost_epoch(&self) -> u64 {
        self.cost_epoch.load(Ordering::Relaxed)
    }

    pub fn stats_manager(&self) -> &Arc<StatisticsManager> {
        &self.stats_manager
    }

    pub fn selectivity_estimator(&self) -> &Arc<SelectivityEstimator> {
        &self.selectivity_estimator
    }

    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expression_context
    }

    pub fn feedback_history(&self) -> Arc<QueryFeedbackHistory> {
        Arc::clone(&self.feedback_history)
    }

    pub fn selectivity_feedback(&self) -> &Arc<SelectivityFeedbackManager> {
        &self.selectivity_feedback
    }

    pub fn cardinality_feedback(&self) -> &Arc<CardinalityFeedbackManager> {
        &self.cardinality_feedback
    }

    pub fn decision_feedback(&self) -> &Arc<DecisionFeedbackStore> {
        &self.decision_feedback
    }

    pub fn set_enable_feedback(&mut self, enable: bool) {
        self.enable_feedback = enable;
        log::info!(
            "Feedback-driven selectivity correction has been {}",
            if enable { "enabled" } else { "disabled" }
        );
    }

    pub fn feedback_enabled(&self) -> bool {
        self.enable_feedback
    }

    pub fn columnar_policy(&self) -> Arc<crate::executor::streaming::chunk::ColumnarPolicy> {
        Arc::clone(&self.columnar_policy)
    }

    pub fn set_metrics_stats(&mut self, stats: Arc<linkrs_metrics::StatsManager>) {
        self.metrics_stats = Some(stats);
    }

    pub fn metrics_stats(&self) -> Option<Arc<linkrs_metrics::StatsManager>> {
        self.metrics_stats.clone()
    }

    pub fn batch_plan_analyzer(&self) -> &BatchPlanAnalyzer {
        &self.batch_plan_analyzer
    }

    pub fn subquery_unnesting_optimizer(&self) -> &SubqueryUnnestingOptimizer {
        &self.subquery_unnesting_optimizer
    }

    pub fn cte_cache_manager(&self) -> &CteCacheManager {
        &self.cte_cache_manager
    }

    pub fn set_cte_cache_stats_manager(&self, stats_manager: Arc<linkrs_metrics::StatsManager>) {
        self.cte_cache_manager.set_stats_manager(stats_manager);
    }

    pub fn set_cost_config(&mut self, config: CostModelConfig) {
        self.cost_config = config;
        self.cost_calculator = Arc::new(CostCalculator::with_config(
            self.stats_manager.clone(),
            self.cost_config,
        ));
        self.space_calculators.write().clear();
        self.cost_epoch.fetch_add(1, Ordering::Relaxed);
        self.batch_plan_analyzer = BatchPlanAnalyzer::new();
        self.subquery_unnesting_optimizer = SubqueryUnnestingOptimizer::new();
        log::info!(
            "Optimizer cost model configuration has been updated: {:?}",
            self.cost_config
        );
    }

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

    pub fn set_max_heuristic_iterations(&mut self, max: usize) {
        self.max_heuristic_iterations = max;
        log::info!(
            "The maximum number of heuristic iterations has been set to {}",
            max
        );
    }

    pub fn set_partitioning_config(&mut self, config: PartitioningConfig) {
        self.partitioning_planner = PartitioningPlanner::new(config);
    }

    pub fn partitioning_config(&self) -> &PartitioningConfig {
        self.partitioning_planner.config()
    }

    pub fn heuristic_batch(&self) -> &crate::optimizer::heuristic::batch::BatchOptimizer {
        self.physical_heuristic.batch()
    }

    pub fn logical_heuristic(&self) -> &LogicalBatchOptimizer {
        &self.logical_heuristic
    }

    pub fn physical_heuristic(&self) -> &PhysicalHeuristicOptimizer {
        &self.physical_heuristic
    }

    pub fn last_batch_statistics(
        &self,
    ) -> Vec<(
        crate::optimizer::heuristic::batch::OptimizationBatch,
        crate::optimizer::heuristic::batch::BatchStatistics,
    )> {
        match self.last_batch_statistics.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}
