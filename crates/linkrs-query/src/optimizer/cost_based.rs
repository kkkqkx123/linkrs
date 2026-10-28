//! Cost-Based Optimization Module
//!
//! Provides query optimization strategies that use statistical information and cost models
//! to make optimization decisions.
//!
//! ## Module Structure
//!
//! - `index` – Index selector
//! - `aggregate_strategy` – Aggregation strategy advisor (notes only)
//! - `join_order` – An optimizer for the order of database table joins
//! - `topn_optimization` – TopN optimizer
//! - `subquery_unnesting` – An optimizer for deassociating subqueries
//! - `expression_precomputation` – Expression precomputation advisor (notes only)
//!
//! ## Usage Examples
//!
//! ```rust
//! use linkrs_query::optimizer::cost::CostCalculator;
//! use linkrs_query::optimizer::cost_based::join_order::JoinOrderOptimizer;
//! use linkrs_query::optimizer::stats::StatisticsManager;
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
pub mod expression_precomputation;
pub mod index;
pub mod index_selection;
pub mod join_order;
pub mod join_order_rewriter;
pub mod ndv;
pub mod precomputation_wiring;
pub mod row_estimates;
pub mod subquery_unnesting;
pub mod topn_optimization;
pub mod topn_wiring;
pub mod traversal;
pub mod traversal_logical;

pub use index::{IndexSelection, IndexSelector, PredicateOperator, PropertyPredicate};

pub use aggregate_strategy::{
    describe_aggregate_strategy, AggregateContext, AggregateStrategy, AggregateStrategyDecision,
    AggregateStrategySelector, SelectionReason as AggregateSelectionReason,
};

pub use join_order::{
    JoinCondition, JoinOrderOptimizer, JoinOrderResult, OptimizationMethod, TableInfo,
};

pub use topn_optimization::{
    SortContext, SortEliminationDecision, SortEliminationOptimizer, SortKeepReason,
    TopNConversionReason,
};

pub use subquery_unnesting::{
    KeepReason, SubqueryUnnestingOptimizer, UnnestDecision, UnnestReason,
};

pub use expression_precomputation::{
    ExpressionPrecomputationOptimizer, NoPrecomputeReason, PrecomputationCandidate,
    PrecomputationDecision, PrecomputeReason,
};
