//! Row-count estimation for the cost-based optimizer.
//!
//! Produces per-node output row estimates for cost-based decisions. The
//! estimates are conservative heuristics driven by the optimizer's statistics
//! when available (tag vertex counts, edge counts, selectivity estimates) and
//! by fixed defaults otherwise. They are consumed by:
//!
//! - the TopN conversion decision (`topn_wiring`),
//! - aggregate strategy notes and join-order review,
//! - learned per-shape cardinality feedback baselines.
//!
//! Estimates travel with the decision that needs them or are written
//! directly onto physical operator specs at assembly; no id-keyed map is
//! carried on the plan.
//!
//! ## Module Structure
//!
//! - `physical` - physical plan walker
//! - `logical` - logical plan walker (shares the same heuristics)
//! - `cardinality` - learned per-shape cardinality feedback
//! - `stats` - statistics-driven helpers shared by both walkers

mod cardinality;
mod logical;
mod physical;
mod stats;

pub(crate) use cardinality::register_plan_estimates;
pub use logical::estimate_node_output_rows_logical;
pub use physical::{estimate_node_output_rows, estimate_node_output_rows_corrected};

#[cfg(test)]
mod tests;
