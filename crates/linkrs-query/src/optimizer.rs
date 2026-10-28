//! Query Optimization Module
//!
//! Provide query optimization capabilities, including the management of statistical information, cost calculation, and optimization strategies.
//!
//! Decision ownership (heuristic vs cost overlap eliminated):
//! - Sort-to-TopN: cost wiring (`cost_based::topn_wiring`) is the sole
//!   decision maker; no heuristic equivalent exists.
//! - Join order: planning enumerates the initial order from the MATCH
//!   query graph with the integer model (`planning::join_order`); the
//!   cost reviewer (`cost_based::join_order` via `join_order_rewriter`)
//!   only re-checks eligible plans. Heuristic join rules are shape
//!   normalization only. The integer model prices graph splits; the
//!   floating-point `CostCalculator` prices the already planned join
//!   chain. The two formulas are intentionally not unified.
//! - Subquery unnesting: cost (`SubqueryUnnestingOptimizer`) decides
//!   pre-CBO; the heuristic rule runs only as a post-CBO fallback over
//!   cost-kept patterns. Cost model tuning itself is out of scope.
//!
//! Freeze: no new feedback types; new decisions reuse the existing
//! cardinality feedback (`stats::feedback::cardinality`). Feedback-system
//! consolidation is deferred by design.
//!
//! ## Module Structure
//!
//! The optimizer is organized into two main optimization phases:
//!
//! ### Heuristic Optimization (`heuristic/`)
//! Rule-based optimizations that always produce better or equivalent plans:
//! - Predicate Pushdown
//! - Projection Pushdown
//! - Elimination Rules
//! - Merge Operations
//! - Limit Pushdown
//!
//! ### Cost-Based Optimization (`cost_based/`)
//! Statistics-driven optimizations that use cost models:
//! - Join Order Optimization
//! - Index Selection
//! - Aggregate Strategy Advice (notes only)
//! - Expression Precomputation Advice (notes only)
//!
//! ### Supporting Modules
//! - `engine` – The globally unique optimizer engine instance with unified optimization interface
//! - `stats` – Statistical information management
//! - `cost` – Cost calculation and estimation
//! - `analysis` – Plan analysis utilities
//! - `decision` – Executed plan decision types (join algorithm choices)
//!
//! ## Usage Examples
//!
//! ```rust
//! use linkrs_query::optimizer::OptimizerEngine;
//!
//! // Create the optimizer engine (global instance)
//! let optimizer = OptimizerEngine::default();
//!
//! // Optimize an execution plan through all enabled phases:
//! // let optimized_plan = optimizer.optimize(plan, None)?;
//! ```

// Core modules
pub mod analysis;
pub mod builder;
pub mod context;
pub mod cost;
pub mod decision;
pub mod engine;
pub mod error;
pub mod factorization;
pub mod partitioning;
pub mod stats;

// Optimization phases
pub mod cost_based; // Cost-based optimization strategies
pub mod heuristic; // Heuristic rewrite rules

// Re-export the main types
pub use builder::OptimizerEngineBuilder;
pub use engine::OptimizerEngine;
pub use error::{CostError, CostResult, OptimizeError, OptimizeResult};
pub use partitioning::{PartitioningConfig, PartitioningDecision, PartitioningPlanner};

pub use stats::{
    CardinalityFeedbackManager, DecisionFeedbackStore, DecorrelationAdvice, EdgeTypeStatistics,
    FeedbackDrivenSelectivity, OperatorFeedback, PropertyStatistics, QueryExecutionFeedback,
    QueryFeedbackHistory, SelectivityFeedbackManager, StatisticsManager, TagStatistics,
};

pub use cost::{CostAssigner, CostCalculator, CostModelConfig, SelectivityEstimator};

// Reexport the analysis module type.
pub use analysis::{
    AggregatedExpressionAnalysis, AnalysisOptions, BatchPlanAnalysis, BatchPlanAnalyzer,
    ExpressionAnalysis, ReferenceCountAnalysis,
};

pub use context::OptimizationContext;

// Re-export cost_based types (formerly strategy)
pub use cost_based::{
    AggregateContext, AggregateSelectionReason, AggregateStrategy, AggregateStrategyDecision,
    AggregateStrategySelector, IndexSelection, IndexSelector, JoinCondition, JoinOrderOptimizer,
    JoinOrderResult, KeepReason, OptimizationMethod, PredicateOperator, PropertyPredicate,
    SortContext, SortEliminationDecision, SortEliminationOptimizer, SortKeepReason,
    SubqueryUnnestingOptimizer, TableInfo, TopNConversionReason, UnnestDecision, UnnestReason,
};

// Re-export the CTE cache types from the cache module.
pub use crate::cache::{
    CteCacheConfig, CteCacheDecision, CteCacheDecisionMaker, CteCacheEntry, CteCacheManager,
    CteCacheStats,
};

// Re-export heuristic types
pub use heuristic::{
    BaseRewriteRule, EliminationRule, MatchedResult, MergeRule, NodeRewriter, PushDownRule,
    RewriteContext, RewriteError, RewriteRule, RuleWrapper, TransformResult,
};

pub use decision::JoinAlgorithm;
