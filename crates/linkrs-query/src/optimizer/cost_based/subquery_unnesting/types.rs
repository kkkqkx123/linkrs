//! Decision and reason types of subquery decorrelation.

/// Maximum constant row bound accepted by [`super::optimizer::SubqueryUnnestingOptimizer::has_bounded_input`]
/// for heuristic decorrelation without statistics.
pub const MAX_BOUNDED_SUBQUERY_ROWS: i64 = 1000;

/// Decentralized decision-making using subqueries
#[derive(Debug, Clone, PartialEq)]
pub enum UnnestDecision {
    /// Convert to SemiJoin/AntiJoin
    ShouldUnnest {
        /// Reason for the decision
        reason: UnnestReason,
        /// Estimated original cost
        original_cost: f64,
        /// Estimated cost after the conversion
        unnested_cost: f64,
    },
    /// Keep the current pattern and apply optimization
    KeepPatternApply {
        /// Reason for retention
        reason: KeepReason,
    },
}

/// Reason for conversion
#[derive(Debug, Clone, PartialEq)]
pub enum UnnestReason {
    /// Simple subquery; the conversion is more efficient.
    SimpleSubquery,
    /// Based on cost analysis
    CostBased,
    /// Measured execution feedback prefers the unnested hash path.
    Empirical,
}

/// Reasons for reservations
#[derive(Debug, Clone, PartialEq)]
pub enum KeepReason {
    /// The subquery is too complex.
    TooComplex,
    /// The subquery contains a non-deterministic function.
    NonDeterministic,
    /// The number of rows estimated by the subquery is too large.
    TooManyRows,
    /// The subquery contains complex conditions.
    ComplexCondition,
    /// The subquery contains an aggregation; semi-join unnesting does not
    /// apply (aggregation over a correlated set is not an equi semi join).
    AggregateSubquery,
    /// Measured execution feedback prefers keeping the nested-loop path.
    EmpiricalKeep,
}
