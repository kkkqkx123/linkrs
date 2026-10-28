use super::expr::BoundExpression;

use crate::parser::ast::{LimitClause, SampleClause, SkipClause};
use linkrs_core::types::OrderDirection;

/// A unified projection item used by RETURN, YIELD, WITH, and COLLECT clauses.
#[derive(Debug, Clone)]
pub struct BoundProjectionItem {
    pub expression: BoundExpression,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BoundOrderByItem {
    pub expression: BoundExpression,
    pub direction: OrderDirection,
}

/// Bound projection shared by RETURN and YIELD clauses.
///
/// The two clauses bind through the same path and differ only in whether
/// undefined variables are rejected eagerly (RETURN) or deferred (YIELD);
/// the bound shape, including the optional sampling clause, is identical.
#[derive(Debug, Clone)]
pub struct BoundProjectionClause {
    pub items: Vec<BoundProjectionItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub sample: Option<SampleClause>,
    /// Bound HAVING filter. Only RETURN binds one (YIELD has no HAVING
    /// syntax); planners consume it exactly like the AST having clause.
    pub having: Option<BoundExpression>,
}

#[derive(Debug, Clone)]
pub struct BoundWhereClause {
    pub condition: BoundExpression,
}

#[derive(Debug, Clone)]
pub struct BoundWithClause {
    pub items: Vec<BoundProjectionItem>,
    pub condition: Option<BoundExpression>,
}
