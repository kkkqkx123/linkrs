//! Translation from logical planner nodes to executable operator specifications.
//!
//! The builders are grouped by domain:
//! - [`source`]: scan / value source specs
//! - [`graph`]: unary, blocking, graph-pattern and recursive-fragment specs
//! - [`join`]: join and apply (correlated subquery) specs
//! - [`ddl`]: sink (write), DDL manage, fulltext and vector specs

mod ddl;
mod graph;
mod join;
mod source;

pub(super) use ddl::*;
pub(super) use graph::*;
pub(super) use join::*;
pub(super) use source::*;

use crate::core::types::expr::Expression;
use crate::query::executor::build_error::PlanBuildError;

/// Convert a [`ContextualExpression`] plan expression into a bare
/// [`Expression`] for embedding into an operator spec.
pub(super) fn contextual_to_expression(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Expression, PlanBuildError> {
    expr.get_expression().ok_or_else(|| {
        PlanBuildError::expression(
            "ContextualExpression",
            0,
            format!("{:?}", expr),
            "Failed to get expression",
        )
    })
}
