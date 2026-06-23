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

// Sorting Executor
pub mod sort;
pub use sort::{SortConfig, SortExecutor, SortKey, SortOrder};

// Limit the execution of the actuator
pub mod limit;
pub use limit::LimitExecutor;

// De-duplication executor
pub mod dedup;
pub use dedup::{DedupExecutor, DedupStrategy};

// Sampling Executor
pub mod sample;
pub use sample::{SampleExecutor, SampleMethod};

// TOP N Optimization
pub mod topn;
pub use topn::TopNExecutor;

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
    UnwindExecutor,
};
