//! Connection Order Optimizer Module
//!
//! Cost-based optimization of the join order to select the optimal sequence of joins for multiple tables
//!
//! ## Algorithm Support
//!
//! Dynamic Programming (DP): Provides an accurate solution for determining the optimal order of connections, suitable for a small number of tables (<=8).
//! Greedy algorithm: A fast method for finding an approximate optimal solution, suitable for dealing with a large number of tables.
//! Left-Deep Tree: A classic representation of a connected graph in the form of a tree structure.
//! “Bushy Tree”: A more flexible version of the connected tree structure.
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

mod condition;
mod optimizer;
mod result;
mod table_info;

pub use condition::JoinCondition;
pub use optimizer::JoinOrderOptimizer;
pub use result::{JoinOrderResult, OptimizationMethod};
pub use table_info::TableInfo;
