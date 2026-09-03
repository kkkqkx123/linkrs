//! Cost-Based Optimization Module
//!
//! Provides query optimization strategies that use statistical information and cost models
//! to make optimization decisions.
//!
//! ## Module Structure
//!
//! - `traversal_start` – Selector for the starting point of the traversal
//! - `index` – Index selector
//! - `aggregate_strategy` – Selector for the aggregation strategy
//! - `join_order` – An optimizer for the order of database table joins
//! - `traversal_direction` – An optimizer for the direction of graph traversal
//! - `bidirectional_traversal` – An optimizer for bidirectional traversal
//! - `topn_optimization` – TopN optimizer
//! - `subquery_unnesting` – An optimizer for deassociating subqueries
//! - `materialization` – The optimization mechanism for materialized CTEs
//! - `memory_budget` – Memory budget allocation
//! - `expression_precomputation` – Expression precomputation optimizer
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb_query::optimizer::cost::CostCalculator;
//! use graphdb_query::optimizer::cost_based::join_order::JoinOrderOptimizer;
//! use graphdb_query::optimizer::stats::StatisticsManager;
//! use std::sync::Arc;
//!
//! let stats_manager = Arc::new(StatisticsManager::new());
//! let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
//! let optimizer = JoinOrderOptimizer::new(cost_calculator);
//!
//! // let decision = optimizer.optimize_join_order(&tables, &conditions);
//! ```
//!
//! The CTE (Common Table Expression) result cache manager has been moved to the `crate::cache` module.

// Module declarations
pub mod aggregate_strategy;
pub mod bidirectional_traversal;
pub mod expression_precomputation;
pub mod index;
pub mod index_selection;
pub mod join_order;
pub mod join_order_rewriter;
pub mod memory_budget;
pub mod ndv;
pub mod precomputation_wiring;
pub mod row_estimates;
pub mod subquery_unnesting;
pub mod topn_optimization;
pub mod topn_wiring;
pub mod traversal;
pub mod traversal_direction;
pub mod traversal_logical;
pub mod traversal_start;

pub use traversal_start::{
    CandidateStart, SelectionReason as TraversalSelectionReason, TraversalStartSelector,
};

pub use index::{IndexSelection, IndexSelector, PredicateOperator, PropertyPredicate};

pub use aggregate_strategy::{
    AggregateContext, AggregateStrategy, AggregateStrategyDecision, AggregateStrategySelector,
    SelectionReason as AggregateSelectionReason,
};

pub use join_order::{
    JoinCondition, JoinOrderOptimizer, JoinOrderResult, OptimizationMethod, TableInfo,
};

pub use bidirectional_traversal::{
    BidirectionalDecision, BidirectionalTraversalOptimizer, DepthAllocationContext,
};

pub use traversal_direction::{
    DegreeInfo, DirectionContext, DirectionSelectionReason, TraversalDirection,
    TraversalDirectionDecision, TraversalDirectionOptimizer,
};

pub use topn_optimization::{
    SortContext, SortEliminationDecision, SortEliminationOptimizer, SortKeepReason,
    TopNConversionReason,
};

pub use subquery_unnesting::{
    KeepReason, SubqueryUnnestingOptimizer, UnnestDecision, UnnestReason,
};

pub use memory_budget::{MemoryBudgetAllocation, MemoryBudgetAllocator, OperatorImplementation};

pub use expression_precomputation::{
    ExpressionPrecomputationOptimizer, NoPrecomputeReason, PrecomputationCandidate,
    PrecomputationDecision, PrecomputeReason,
};

// Re-export the CTE cache type from the cache module (for backward compatibility)
pub use crate::cache::{
    CteCacheConfig, CteCacheDecision, CteCacheDecisionMaker, CteCacheEntry, CteCacheManager,
    CteCacheStats,
};
