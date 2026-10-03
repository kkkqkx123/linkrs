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

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

use crate::optimizer::heuristic::batch::{BatchStatistics, OptimizationBatch};
use crate::optimizer::heuristic::{LogicalBatchOptimizer, PhysicalHeuristicOptimizer};
use crate::optimizer::partitioning::PartitioningPlanner;
use crate::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
use crate::optimizer::stats::feedback::decision::DecisionFeedbackStore;
use crate::optimizer::stats::feedback::history::QueryFeedbackHistory;
use crate::optimizer::stats::feedback::selectivity::SelectivityFeedbackManager;
use crate::optimizer::stats::feedback::trigger::AutoFeedbackTrigger;
use crate::optimizer::{
    BatchPlanAnalyzer, CostCalculator, CostModelConfig, CteCacheManager, SelectivityEstimator,
    StatisticsManager, SubqueryUnnestingOptimizer,
};
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;

mod config;
mod factorization;
mod feedback;
mod optimizer;

#[cfg(test)]
#[allow(unused_imports)]
pub use config::*;
#[allow(unused_imports)]
pub use factorization::*;
#[allow(unused_imports)]
pub use optimizer::*;

#[derive(Debug)]
pub struct OptimizerEngine {
    expression_context: Arc<ExpressionAnalysisContext>,
    stats_manager: Arc<StatisticsManager>,
    cte_cache_manager: Arc<CteCacheManager>,
    cost_calculator: Arc<CostCalculator>,
    selectivity_estimator: Arc<SelectivityEstimator>,
    batch_plan_analyzer: BatchPlanAnalyzer,
    subquery_unnesting_optimizer: SubqueryUnnestingOptimizer,
    cost_config: CostModelConfig,
    space_cost_configs: parking_lot::RwLock<HashMap<String, CostModelConfig>>,
    space_calculators: parking_lot::RwLock<HashMap<String, Arc<CostCalculator>>>,
    cost_epoch: AtomicU64,
    logical_heuristic: LogicalBatchOptimizer,
    physical_heuristic: PhysicalHeuristicOptimizer,
    last_batch_statistics: Mutex<Vec<(OptimizationBatch, BatchStatistics)>>,
    partitioning_planner: PartitioningPlanner,
    enable_heuristic: bool,
    max_heuristic_iterations: usize,
    feedback_history: Arc<QueryFeedbackHistory>,
    selectivity_feedback: Arc<SelectivityFeedbackManager>,
    cardinality_feedback: Arc<CardinalityFeedbackManager>,
    decision_feedback: Arc<DecisionFeedbackStore>,
    feedback_trigger: AutoFeedbackTrigger,
    enable_feedback: bool,
    columnar_policy: Arc<crate::executor::streaming::chunk::ColumnarPolicy>,
    metrics_stats: Option<Arc<graphdb_metrics::StatsManager>>,
}

impl Default for OptimizerEngine {
    fn default() -> Self {
        Self::new(CostModelConfig::default())
    }
}
