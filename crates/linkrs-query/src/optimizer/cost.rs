//! Cost Calculation Module
//!
//! Provide the cost calculation functionality required by the query optimizer.
//!
//! ## Module Structure
//!
//! “Calculator” refers to a cost estimator that calculates the costs of various operations.
//! “Selectivity” refers to an estimator that measures the selectiveness of the query conditions used in a data retrieval process. In other words, it assesses how well the query criteria filter out irrelevant data and only retrieve the relevant records. This property is particularly important in database systems, as it can significantly impact the performance and efficiency of data queries.
//! `config` – Configuration of the cost model
//! “Assigner” is a cost assigner that assigns costs to the nodes in the execution plan.
//! “Estimate” refers to the result of the cost estimation for a node.
//! `child_accessor` – Accessor for child nodes
//! `expression_parser` – An expression parser
//! `node_estimators` – Estimators for different types of nodes

pub mod assigner;
pub mod calculator;
pub mod child_accessor;
pub mod config;
pub mod estimate;
pub mod expression_parser;
pub mod node_estimators;
pub mod selectivity;

pub use assigner::CostAssigner;
pub use calculator::CostCalculator;
pub use config::{CostModelConfig, StrategyThresholds};
pub use estimate::NodeCostEstimate;
pub use selectivity::SelectivityEstimator;
