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

/// Shared unnesting decision core operating on extracted shape facts.
///
/// Physical and logical adapters extract determinism, complexity, shape and
/// row counts from their own node types and call through here, so cost
/// comparison cannot drift between the two trees.
pub fn pattern_apply_cost(
    left_rows: f64,
    subquery_rows: u64,
    measured_per_row_cost: Option<f64>,
) -> f64 {
    match measured_per_row_cost {
        Some(cost) => left_rows * cost * 0.001,
        None => left_rows * (subquery_rows as f64 * 0.1),
    }
}

/// Hash join cost shared by both unnesting paths.
pub fn hash_join_cost(left_rows: f64, subquery_rows: u64) -> f64 {
    let right_rows = subquery_rows as f64;
    right_rows + left_rows * 0.5
}

/// Whether the condition is a simple equality, AND chain or OR of equalities.
pub fn is_simple_equality_condition(expr: &linkrs_core::Expression) -> bool {
    use linkrs_core::types::operators::BinaryOperator;
    use linkrs_core::Expression;
    match expr {
        Expression::Binary { op, left, right } => match op {
            BinaryOperator::Equal => {
                is_simple_expression(left.as_ref()) && is_simple_expression(right.as_ref())
            }
            BinaryOperator::And | BinaryOperator::Or => {
                is_simple_equality_condition(left.as_ref())
                    && is_simple_equality_condition(right.as_ref())
            }
            _ => false,
        },
        _ => false,
    }
}

/// Whether the expression is a literal, variable or property reference.
pub fn is_simple_expression(expr: &linkrs_core::Expression) -> bool {
    use linkrs_core::Expression;
    matches!(
        expr,
        Expression::Literal(_) | Expression::Variable(_) | Expression::Property { .. }
    )
}
