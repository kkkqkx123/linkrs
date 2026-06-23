//! Relational Algebra Executor Module
//!
//! This module implements core relational algebra operations:
//! - Selection (σ): Filter, Having
//! - Projection (π): Project
//! - Join (⋈): Inner, Left, Full Outer, Cross
//! - Set Operations (∪, ∩, −): Union, Intersect, Minus
//! - Aggregation (γ): Aggregate, GroupBy

pub mod aggregation;
pub mod join;
pub mod projection;
pub mod selection;
pub mod set_operations;

// Re-export selection executors
pub use selection::FilterExecutor;

// Re-export projection executors
pub use projection::{ProjectExecutor, ProjectionColumn};

// Re-export aggregation executors
pub use aggregation::{
    AggregateExecutor, AggregateFunctionSpec, GroupAggregateState, GroupByExecutor, HavingExecutor,
};

// Re-export join executors
pub use join::{
    CrossJoinExecutor, FullOuterJoinExecutor, HashInnerJoinExecutor, HashLeftJoinExecutor,
    InnerJoinExecutor, LeftJoinExecutor,
};

// Re-export set operation executors
pub use set_operations::{
    IntersectExecutor, MinusExecutor, SetExecutor, UnionAllExecutor, UnionExecutor,
};
