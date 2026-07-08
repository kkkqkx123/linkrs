//! Optimizer Engine Builder Module
//!
//! Provides a builder pattern for creating OptimizerEngine instances,
//! improving flexibility and reducing coupling.

use std::sync::Arc;

use crate::query::optimizer::OptimizerEngine;
use crate::query::optimizer::{
    CostModelConfig, CteCacheManager, SelectivityFeedbackManager, StatisticsManager,
};
use crate::query::validator::context::ExpressionAnalysisContext;

/// Builder for creating OptimizerEngine instances
pub struct OptimizerEngineBuilder {
    cost_config: Option<CostModelConfig>,
    expression_context: Option<Arc<ExpressionAnalysisContext>>,
    stats_manager: Option<Arc<StatisticsManager>>,
    selectivity_feedback_manager: Option<Arc<SelectivityFeedbackManager>>,
    cte_cache_manager: Option<Arc<CteCacheManager>>,
}

impl Default for OptimizerEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl OptimizerEngineBuilder {
    /// Create a new builder with default configuration.
    pub fn new() -> Self {
        Self {
            cost_config: None,
            expression_context: None,
            stats_manager: None,
            selectivity_feedback_manager: None,
            cte_cache_manager: None,
        }
    }

    /// Set the cost model configuration.
    pub fn with_cost_config(mut self, config: CostModelConfig) -> Self {
        self.cost_config = Some(config);
        self
    }

    /// Set the expression analysis context.
    pub fn with_expression_context(mut self, ctx: Arc<ExpressionAnalysisContext>) -> Self {
        self.expression_context = Some(ctx);
        self
    }

    /// Set the statistics manager.
    pub fn with_stats_manager(mut self, manager: Arc<StatisticsManager>) -> Self {
        self.stats_manager = Some(manager);
        self
    }

    /// Set the selectivity feedback manager.
    pub fn with_selectivity_feedback_manager(
        mut self,
        manager: Arc<SelectivityFeedbackManager>,
    ) -> Self {
        self.selectivity_feedback_manager = Some(manager);
        self
    }

    /// Set the CTE cache manager.
    pub fn with_cte_cache_manager(mut self, manager: Arc<CteCacheManager>) -> Self {
        self.cte_cache_manager = Some(manager);
        self
    }

    /// Build the OptimizerEngine with the configured settings.
    pub fn build(self) -> OptimizerEngine {
        let cost_config = self.cost_config.unwrap_or_default();
        let expression_context = self
            .expression_context
            .unwrap_or_else(|| Arc::new(ExpressionAnalysisContext::new()));
        let stats_manager = self
            .stats_manager
            .unwrap_or_else(|| Arc::new(StatisticsManager::new()));
        let selectivity_feedback_manager = self
            .selectivity_feedback_manager
            .unwrap_or_else(|| Arc::new(SelectivityFeedbackManager::new()));
        let cte_cache_manager = self
            .cte_cache_manager
            .unwrap_or_else(|| Arc::new(CteCacheManager::new()));

        OptimizerEngine::with_components(
            expression_context,
            stats_manager,
            selectivity_feedback_manager,
            cte_cache_manager,
            cost_config,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_default() {
        let builder = OptimizerEngineBuilder::default();
        let engine = builder.build();
        assert_eq!(engine.cost_config(), &CostModelConfig::default());
    }

    #[test]
    fn test_builder_with_cost_config() {
        let config = CostModelConfig::for_ssd();
        let builder = OptimizerEngineBuilder::new().with_cost_config(config);
        let engine = builder.build();
        assert_eq!(engine.cost_config(), &config);
    }

    #[test]
    fn test_builder_with_expression_context() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let builder = OptimizerEngineBuilder::new().with_expression_context(ctx.clone());
        let engine = builder.build();
        assert!(Arc::ptr_eq(engine.expression_context(), &ctx));
    }

    #[test]
    fn test_builder_with_stats_manager() {
        let stats = Arc::new(StatisticsManager::new());
        let builder = OptimizerEngineBuilder::new().with_stats_manager(stats.clone());
        let engine = builder.build();
        assert_eq!(
            engine.stats_manager().as_ref() as *const _,
            stats.as_ref() as *const _
        );
    }

    #[test]
    fn test_builder_chain() {
        let config = CostModelConfig::for_ssd();
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let stats = Arc::new(StatisticsManager::new());

        let engine = OptimizerEngineBuilder::new()
            .with_cost_config(config)
            .with_expression_context(ctx)
            .with_stats_manager(stats)
            .build();

        assert_eq!(engine.cost_config(), &config);
    }
}
