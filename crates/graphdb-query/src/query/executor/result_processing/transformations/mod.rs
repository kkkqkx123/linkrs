//! Data Conversion Executor Module
//!
//! Include all executors related to data conversion, including:
//! Assign (variable assignment)
//! AppendVertices (Adding Vertices)
//! "Unwind" (list expansion) – This phrase refers to the process of expanding or displaying all the items in a list in detail. For example, if you have a list with only a few items visible at the top of the screen, clicking on the "Unwind" button or option will show all the items in the list.
//! PatternApply (Pattern Matching)
//! RollUpApply (aggregation operation)
//!
//! Corresponding NebulaGraph implementation:
//! nebula-3.8.0/src/graph/executor/query/

//! Macros for reducing boilerplate code
#[macro_use]
mod macros;

/// Helper functions for transformation executors
pub mod helpers;

// Variable Assignment Executor
pub mod assign;
pub use assign::AssignExecutor;

// List Expansion Executor
pub mod unwind;
pub use unwind::UnwindExecutor;

// Additional vertex executor
pub mod append_vertices;
pub use append_vertices::AppendVerticesExecutor;

// Pattern matching executor
pub mod pattern_apply;
pub use pattern_apply::PatternApplyExecutor;

// Aggregation Operation Executor
pub mod rollup_apply;
pub use rollup_apply::RollUpApplyExecutor;
