//! Result Processing Executor Module
//!
//! This module handles final result processing and output optimization:
//! - Ordering: Sort, TopN
//! - Limiting: Limit, Offset
//! - Deduplication: DISTINCT
//! - Sampling: Random sampling
//! - Transformations: Data format conversions

// Aggregated data status (refer to nebula-graph AggData)
pub mod agg_data;
pub use agg_data::AggData;

// Aggregation Function Manager (refer to nebula-graph AggFunctionManager)
pub mod agg_function_manager;
pub use agg_function_manager::AggFunctionManager;

// Data conversion operations
// These actuators perform data conversion operations, including:
// Assign (variable assignment)
// "Unwind" (list expansion)
// AppendVertices (Adding Vertices)
// PatternApply (Pattern matching)
// RollUpApply (Aggregation Operation)
pub mod transformations;
pub use transformations::{
    AppendVerticesExecutor, AssignExecutor, PatternApplyExecutor, RollUpApplyExecutor,
};
