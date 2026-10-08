//! Row-count estimation for the cost-based optimizer.
//!
//! Produces per-node output row estimates for the cost-based phase. The
//! estimates are conservative heuristics driven by the optimizer's statistics
//! when available (tag vertex counts, edge counts, selectivity estimates) and
//! by fixed defaults otherwise. They are consumed by:
//!
//! - the TopN conversion decision (`topn_wiring`),
//! - the `estimated_rows` writeback pass (per-logical-node map).
//!
//! ## Module Structure
//!
//! - `physical` - physical plan walker
//! - `logical` - logical plan walker (shares the same heuristics)
//! - `cardinality` - learned per-shape cardinality feedback
//! - `stats` - statistics-driven helpers shared by both walkers

use std::collections::HashMap;

use crate::optimizer::cost::SelectivityEstimator;
use crate::optimizer::stats::StatsView;
use crate::planning::plan::PlanNodeEnum;

mod cardinality;
mod logical;
mod physical;
mod stats;

pub(crate) use cardinality::register_plan_estimates;
pub use logical::estimate_node_output_rows_logical;
pub use physical::{estimate_node_output_rows, estimate_node_output_rows_corrected};

/// Collect output row estimates for every node in the plan, keyed by node id.
///
/// The map is attached to the optimized [`ExecutionPlan`](crate::planning::plan::ExecutionPlan)
/// and later written into physical operator specs by the `estimated_rows`
/// metadata pass (matched by `logical_node_id`).
pub fn collect_node_row_estimates(
    root: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
) -> HashMap<i64, u64> {
    let mut estimates = HashMap::new();
    collect_node_row_estimates_recursive(root, stats, selectivity, &mut estimates);
    estimates
}

fn collect_node_row_estimates_recursive(
    node: &PlanNodeEnum,
    stats: &StatsView,
    selectivity: &SelectivityEstimator,
    estimates: &mut HashMap<i64, u64>,
) {
    for child in node.children() {
        collect_node_row_estimates_recursive(child, stats, selectivity, estimates);
    }
    let estimate = estimate_node_output_rows(node, stats, selectivity);
    estimates.insert(node.id(), estimate);
}

#[cfg(test)]
mod tests;
