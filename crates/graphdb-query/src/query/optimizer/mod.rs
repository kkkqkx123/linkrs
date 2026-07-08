//! Query Optimization Module
//!
//! Provide query optimization capabilities, including the management of statistical information, cost calculation, and optimization strategies.
//!
//! ## Module Structure
//!
//! The optimizer is organized into two main optimization phases:
//!
//! ### Phase 1: Heuristic Optimization (`heuristic/`)
//! Rule-based optimizations that always produce better or equivalent plans:
//! - Predicate Pushdown
//! - Projection Pushdown
//! - Elimination Rules
//! - Merge Operations
//! - Limit Pushdown
//!
//! ### Phase 2: Cost-Based Optimization (`cost_based/`)
//! Statistics-driven optimizations that use cost models:
//! - Join Order Optimization
//! - Index Selection
//! - Traversal Start/Direction Selection
//! - Aggregate Strategy Selection
//! - Materialization Decision
//!
//! ### Supporting Modules
//! - `engine` – The globally unique optimizer engine instance with unified optimization interface
//! - `stats` – Statistical information management
//! - `cost` – Cost calculation and estimation
//! - `analysis` – Plan analysis utilities
//! - `decision` – Optimization decision types
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::OptimizerEngine;
//!
//! // Create the optimizer engine (global instance)
//! let optimizer = OptimizerEngine::default();
//!
//! // Optimize an execution plan through all enabled phases
//! let optimized_plan = optimizer.optimize(plan)?;
//! ```

// Core modules
pub mod analysis;
pub mod builder;
pub mod context;
pub mod cost;
pub mod decision;
pub mod engine;
pub mod error;
pub mod stats;

// Optimization phases
pub mod cost_based; // Cost-based optimization strategies
pub mod heuristic; // Heuristic rewrite rules

// Re-export the main types
pub use builder::OptimizerEngineBuilder;
pub use engine::OptimizerEngine;
pub use error::{CostError, CostResult, OptimizeError, OptimizeResult};

pub use stats::{
    EdgeTypeStatistics, ExecutionFeedbackCollector, FeedbackDrivenSelectivity, OperatorFeedback,
    PropertyStatistics, QueryExecutionFeedback, QueryFeedbackHistory, SelectivityFeedbackManager,
    StatisticsManager, TagStatistics,
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
    AggregateStrategySelector, CandidateStart, CteCacheConfig, CteCacheDecision,
    CteCacheDecisionMaker, CteCacheEntry, CteCacheManager, CteCacheStats, DegreeInfo,
    DirectionContext, DirectionSelectionReason, IndexSelection, IndexSelector, JoinCondition,
    JoinOrderOptimizer, JoinOrderResult, KeepReason, MaterializationDecision,
    MaterializationOptimizer, MaterializeReason, NoMaterializeReason, OptimizationMethod,
    PredicateOperator, PropertyPredicate, SortContext, SortEliminationDecision,
    SortEliminationOptimizer, SortKeepReason, SubqueryUnnestingOptimizer, TableInfo,
    TopNConversionReason, TraversalDirection, TraversalDirectionDecision,
    TraversalDirectionOptimizer, TraversalSelectionReason, TraversalStartSelector, UnnestDecision,
    UnnestReason,
};

// Re-export heuristic types
pub use heuristic::{
    BaseRewriteRule, EliminationRule, HeuristicRule, HeuristicRuleAdapter, IntoOptRule,
    MatchedResult, MergeRule, PlanRewriter, PushDownRule, RewriteContext, RewriteError,
    RewriteRule, RuleWrapper, TransformResult,
};

pub use decision::{
    AccessPath, EntityIndexChoice, EntityType, IndexChoice, IndexSelectionDecision, JoinAlgorithm,
    JoinOrderDecision, OptimizationDecision, RewriteRuleId, TraversalStartDecision,
};
