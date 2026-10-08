//! Query Plan Explanation Module
//!
//! This module provides functionality to describe and format execution plans
//! for human-readable output (EXPLAIN command).

pub mod describe_visitor;
pub mod description;

pub use describe_visitor::DescribeVisitor;
pub use description::{
    Pair, PlanDescription, PlanNodeBranchInfo, PlanNodeDescription, ProfilingStats,
};
