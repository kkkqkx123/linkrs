//! Required-property demand analysis (typed property pruning)
//!
//! Propagates property requirements top-down through the plan tree: each
//! consumer declares which `var.prop` columns it needs, and every node
//! records the requirement set its consumers demand of its output. Graph
//! source operators (`GetVertices` / `GetNeighbors` / `ScanVertices` /
//! `ScanEdges` / `GetEdges` / `AppendVertices`) consume the requirement for
//! their binding variable to narrow `projected_properties`.
//!
//! # Typed collection rule
//!
//! Only `Expression::Property { object: Variable(v), property }` references
//! contribute a prunable requirement. Every other occurrence of a variable
//! (bare reference, opaque property object, function argument, alias) marks
//! the variable as *full-value*: its whole column set must be preserved, so
//! the analyzer never narrows a binding that is consumed in a non-prunable
//! way. This keeps the narrowing sound for computed expressions, aliases,
//! and path semantics.

mod analyzer;
mod map;
mod ref_collector;
mod requirement;

pub use analyzer::{binding_var, RequiredPropertyAnalyzer};
pub use map::RequiredPropertiesMap;
pub use requirement::PropertyRequirement;
