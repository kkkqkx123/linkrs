//! JOIN Executor Module
//!
//! Includes all executors related to JOIN operations, including:
//! InnerJoin (inner join)
//! LeftJoin (left outer join)
//! FullOuterJoin
//! CrossJoin/CartesianProduct (Cartesian product)
//!
//! The implementation of the `join` operation is based on the `nebula-graph` framework, and the performance is optimized by using the hash join algorithm.
//!
//! The RightJoin has been removed because the same functionality can be achieved by swapping the order of the tables using a LeftJoin.

pub mod base_join;
pub mod cross_join;
pub mod full_outer_join;
pub mod hash_table;
pub mod inner_join;
pub mod join_key_evaluator;
pub mod left_join;

// Re-export the main types
pub use base_join::BaseJoinExecutor;
pub use cross_join::CrossJoinExecutor;
pub use full_outer_join::FullOuterJoinExecutor;
pub use hash_table::{HashTableBuilder, JoinKey};
pub use inner_join::{HashInnerJoinExecutor, InnerJoinConfig, InnerJoinExecutor};
pub use join_key_evaluator::JoinKeyEvaluator;
pub use left_join::{HashLeftJoinExecutor, LeftJoinConfig, LeftJoinExecutor};

// Import the `JoinType` from the `core` module.
pub use crate::core::types::JoinType;

// Re-export the ExpressionContextStruct alias for use by all join modules
pub use crate::query::validator::context::ExpressionAnalysisContext as ExpressionContextStruct;

/// Configuration of the Join operation
#[derive(Debug, Clone)]
pub struct JoinConfig {
    /// Join type
    pub join_type: JoinType,
    /// Left input variable name
    pub left_var: String,
    /// Enter the variable name on the right.
    pub right_var: String,
    /// List of connection key expressions (left table)
    pub left_keys: Vec<String>,
    /// List of connection key expressions (right table)
    pub right_keys: Vec<String>,
    /// Column names
    pub output_columns: Vec<String>,
    /// Should parallel processing be enabled?
    pub enable_parallel: bool,
}

impl JoinConfig {
    /// Create an inner join configuration.
    pub fn inner_join(
        left_var: String,
        right_var: String,
        left_keys: Vec<String>,
        right_keys: Vec<String>,
        output_columns: Vec<String>,
    ) -> Self {
        Self {
            join_type: JoinType::Inner,
            left_var,
            right_var,
            left_keys,
            right_keys,
            output_columns,
            enable_parallel: false,
        }
    }

    /// Create a left outer join configuration.
    pub fn left_join(
        left_var: String,
        right_var: String,
        left_keys: Vec<String>,
        right_keys: Vec<String>,
        output_columns: Vec<String>,
    ) -> Self {
        Self {
            join_type: JoinType::Left,
            left_var,
            right_var,
            left_keys,
            right_keys,
            output_columns,
            enable_parallel: false,
        }
    }
}
