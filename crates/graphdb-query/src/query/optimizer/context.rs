//! Optimization Context Module
//!
//! Provides a unified context for query optimization, encapsulating all shared data needed during optimization.
//! This reduces coupling between components and simplifies dependency management.

use std::sync::Arc;

use crate::query::optimizer::analysis::{
    BatchPlanAnalysis, ExpressionAnalysis, ReferenceCountAnalysis,
};
use crate::query::optimizer::cost::{CostCalculator, CostModelConfig, SelectivityEstimator};
use crate::query::optimizer::stats::StatisticsManager;
use crate::query::validator::context::ExpressionAnalysisContext;

/// Optimization Context
///
/// Encapsulates all shared data needed during query optimization, providing a unified interface
/// for all optimization strategies and analyzers.
#[derive(Clone)]
pub struct OptimizationContext {
    /// Statistics information manager
    stats_manager: Arc<StatisticsManager>,
    /// Cost calculator
    cost_calculator: Arc<CostCalculator>,
    /// Selectivity estimator
    selectivity_estimator: Arc<SelectivityEstimator>,
    /// Cost model configuration
    cost_config: CostModelConfig,
    /// Expression analysis context (shared across different stages)
    expression_context: Arc<ExpressionAnalysisContext>,
    /// Cached reference count analysis (computed once per optimization)
    reference_count_analysis: Option<ReferenceCountAnalysis>,
    /// Cached expression analysis (computed once per optimization)
    expression_analysis: Option<ExpressionAnalysis>,
    /// Cached batch plan analysis (computed once per optimization)
    batch_plan_analysis: Option<BatchPlanAnalysis>,
}

impl OptimizationContext {
    /// Create a new optimization context.
    pub fn new(
        stats_manager: Arc<StatisticsManager>,
        cost_calculator: Arc<CostCalculator>,
        selectivity_estimator: Arc<SelectivityEstimator>,
        cost_config: CostModelConfig,
        expression_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            stats_manager,
            cost_calculator,
            selectivity_estimator,
            cost_config,
            expression_context,
            reference_count_analysis: None,
            expression_analysis: None,
            batch_plan_analysis: None,
        }
    }

    /// Get the statistics manager.
    pub fn stats_manager(&self) -> &Arc<StatisticsManager> {
        &self.stats_manager
    }

    /// Get the cost calculator.
    pub fn cost_calculator(&self) -> &Arc<CostCalculator> {
        &self.cost_calculator
    }

    /// Get the selectivity estimator.
    pub fn selectivity_estimator(&self) -> &Arc<SelectivityEstimator> {
        &self.selectivity_estimator
    }

    /// Get the cost model configuration.
    pub fn cost_config(&self) -> &CostModelConfig {
        &self.cost_config
    }

    /// Get the expression analysis context.
    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expression_context
    }

    /// Set the cached reference count analysis.
    pub fn set_reference_count_analysis(&mut self, analysis: ReferenceCountAnalysis) {
        self.reference_count_analysis = Some(analysis);
    }

    /// Get the cached reference count analysis.
    pub fn reference_count_analysis(&self) -> Option<&ReferenceCountAnalysis> {
        self.reference_count_analysis.as_ref()
    }

    /// Set the cached expression analysis.
    pub fn set_expression_analysis(&mut self, analysis: ExpressionAnalysis) {
        self.expression_analysis = Some(analysis);
    }

    /// Get cached expression analysis.
    pub fn expression_analysis(&self) -> Option<&ExpressionAnalysis> {
        self.expression_analysis.as_ref()
    }

    /// Set the cached batch plan analysis.
    pub fn set_batch_plan_analysis(&mut self, analysis: BatchPlanAnalysis) {
        self.batch_plan_analysis = Some(analysis);
    }

    /// Get cached batch plan analysis.
    pub fn batch_plan_analysis(&self) -> Option<&BatchPlanAnalysis> {
        self.batch_plan_analysis.as_ref()
    }

    /// Clear all cached analysis results.
    pub fn clear_cache(&mut self) {
        self.reference_count_analysis = None;
        self.expression_analysis = None;
        self.batch_plan_analysis = None;
    }
}

use crate::query::optimizer::OptimizerEngine;

impl From<&OptimizerEngine> for OptimizationContext {
    fn from(engine: &OptimizerEngine) -> Self {
        Self::new(
            engine.stats_manager().clone(),
            engine.cost_calculator().clone(),
            engine.selectivity_estimator().clone(),
            *engine.cost_config(),
            engine.expression_context().clone(),
        )
    }
}

impl From<&Arc<OptimizerEngine>> for OptimizationContext {
    fn from(engine: &Arc<OptimizerEngine>) -> Self {
        Self::from(engine.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimization_context_creation() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_config = CostModelConfig::default();
        let cost_calculator = Arc::new(CostCalculator::with_config(
            stats_manager.clone(),
            cost_config,
        ));
        let selectivity_estimator = Arc::new(SelectivityEstimator::new(stats_manager.clone()));
        let expression_context = Arc::new(ExpressionAnalysisContext::new());

        let ctx = OptimizationContext::new(
            stats_manager,
            cost_calculator,
            selectivity_estimator,
            cost_config,
            expression_context,
        );

        assert!(ctx.reference_count_analysis().is_none());
        assert!(ctx.expression_analysis().is_none());
    }

    #[test]
    fn test_optimization_context_cache() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_config = CostModelConfig::default();
        let cost_calculator = Arc::new(CostCalculator::with_config(
            stats_manager.clone(),
            cost_config,
        ));
        let selectivity_estimator = Arc::new(SelectivityEstimator::new(stats_manager.clone()));
        let expression_context = Arc::new(ExpressionAnalysisContext::new());

        let mut ctx = OptimizationContext::new(
            stats_manager,
            cost_calculator,
            selectivity_estimator,
            cost_config,
            expression_context,
        );

        let ref_analysis = ReferenceCountAnalysis::new();
        ctx.set_reference_count_analysis(ref_analysis.clone());
        assert!(ctx.reference_count_analysis().is_some());

        let expr_analysis = ExpressionAnalysis::default();
        ctx.set_expression_analysis(expr_analysis.clone());
        assert!(ctx.expression_analysis().is_some());

        ctx.clear_cache();
        assert!(ctx.reference_count_analysis().is_none());
        assert!(ctx.expression_analysis().is_none());
    }
}
