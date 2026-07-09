//! Relational Algebra Executor Module
//!
//! This module implements core relational algebra operations:
//! - Selection (σ): Filter, Having
//! - Join (⋈): Inner, Left, Full Outer, Cross
//! - Set Operations (∪, ∩, −): Union, Intersect, Minus
//!
//! Note: Aggregation and Window operations are now handled by StreamingExecutor.

pub mod join;
pub mod selection;
pub mod set_operations;

// Re-export selection executors
pub use selection::FilterExecutor;

// Re-export join executors
pub use join::{
    CrossJoinExecutor, FullOuterJoinExecutor, HashInnerJoinExecutor, HashLeftJoinExecutor,
    InnerJoinExecutor, LeftJoinExecutor,
};

// Re-export set operation executors
pub use set_operations::{
    IntersectExecutor, MinusExecutor, SetExecutor, UnionAllExecutor, UnionExecutor,
};

// Legacy type stubs for backward compatibility (if needed elsewhere)
pub use crate::core::types::JoinType;

// Placeholder stubs for old exports that are no longer available
// These are kept only for avoiding compilation errors in tests or other code
// New code should use StreamingExecutor instead
pub struct ProjectExecutor;
pub struct ProjectionColumn;
pub struct AggregateExecutor;
pub struct AggregateFunctionSpec;
pub struct GroupAggregateState;
pub struct GroupByExecutor;
pub struct HavingExecutor;
pub struct WindowExecutor;
