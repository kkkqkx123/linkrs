//! Aggregation Policy Selector Module
//!
//! Cost-based selection of aggregation strategies: determining the optimal approach between hash aggregation and sort aggregation
//!
//! ## Usage Examples
//!
//! ```rust
//! use linkrs_query::optimizer::cost::CostCalculator;
//! use linkrs_query::optimizer::cost_based::AggregateStrategySelector;
//! use linkrs_query::optimizer::stats::StatisticsManager;
//! use std::sync::Arc;
//!
//! let stats_manager = Arc::new(StatisticsManager::new());
//! let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
//! let selector = AggregateStrategySelector::new(cost_calculator);
//!
//! // let decision = selector.select_strategy("test", &context);
//! ```

mod context;
mod selector;
mod strategy;

pub use context::AggregateContext;
pub use selector::{walk_aggregate_strategies_logical, AggregateStrategySelector};
pub use strategy::{AggregateStrategy, AggregateStrategyDecision, SelectionReason};
