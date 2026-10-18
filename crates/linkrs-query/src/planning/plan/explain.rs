//! Query Plan Explanation Module
//!
//! Planning side retains only the plan description DTOs (`description`).
//! The legacy logical-tree `DescribeVisitor` was removed: the live builder
//! is `executor::explain::physical_plan_explain::physical_plan_to_plan_description`
//! over the physical arena. Row estimates (`est_rows`) are sourced there from
//! `op_spec.estimated_cardinality` at build time; actual rows are overlaid
//! later as `ProfilingStats.rows` by the pipeline assembly.

pub mod description;

pub use description::{
    Pair, PlanDescription, PlanNodeBranchInfo, PlanNodeDescription, ProfilingStats,
};
