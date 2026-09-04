//! Logical node struct definitions.
//!
//! Each module defines logical variants of the physical plan nodes,
//! using `LogicalNodeEnum` as the child type instead of `PlanNodeEnum`.

pub mod access;
pub mod algorithm;
pub mod control_flow;
pub mod dml;
pub mod flatten;
pub mod graph_ops;
pub mod join;
pub mod operation;
pub mod search;
pub mod traversal;
pub mod wco_intersect;
