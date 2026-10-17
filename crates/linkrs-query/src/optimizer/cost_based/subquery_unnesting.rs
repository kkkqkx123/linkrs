//! Subquery decorrelation optimization module
//!
//! "Analysis-based subquery decorrelation optimization strategy" — This
//! strategy converts simple PatternApply subqueries into SemiJoin/AntiJoin
//! operations (semi-join unnesting). PatternApply is the correlated
//! subquery operator: the left input is the main pipeline, the right input
//! is the per-row subquery. Unnesting replaces the apply (per-row
//! execution) with a single build/probe pass over both inputs.
//!
//! ## Optimization Strategies
//!
//! Convert the eligible PatternApply subquery into a SemiJoin (EXISTS) or
//! AntiJoin (NOT EXISTS) node. Avoid executing subqueries repeatedly.
//!
//! ## Applicable Conditions
//!
//! 1. The right input for PatternApply is a simple query (single-table
//!    scan — optionally a simple two-table inner join tree — wrapped in
//!    equality or disjunctive-equality filtering, optionally wrapped in a
//!    constant Limit).
//! 2. The filtering conditions are deterministic (excluding rand(), now(),
//!    etc.)
//! 3. The complexity of the expressions should be less than 50 (avoid using
//!    complex expressions).
//! 4. The subquery estimates that the number of rows is less than 1000
//!    (based on statistical information; the stat-free heuristic gate uses
//!    the constant [`MAX_BOUNDED_SUBQUERY_ROWS`] limit cap instead).
//!
//! ## Usage Examples
//!
//! ```rust
//! use linkrs_query::optimizer::cost_based::SubqueryUnnestingOptimizer;
//!
//! let optimizer = SubqueryUnnestingOptimizer::new();
//!
//! // let decision = optimizer.should_unnest(&pattern_apply, &analysis, &stats_view, &selectivity);
//! ```

mod logical;
mod optimizer;
mod types;

pub use logical::{build_semi_join_from_pattern_apply_logical, unnest_pattern_applies_logical};
pub use optimizer::SubqueryUnnestingOptimizer;
pub use types::{KeepReason, UnnestDecision, UnnestReason, MAX_BOUNDED_SUBQUERY_ROWS};
