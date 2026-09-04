//! Expression validation trait and walker.
//!
//! This module provides a generic `ExprValidator` trait and `walk_expr` function
//! for performing single-pass expression tree validation with depth tracking and
//! error handling.

use crate::error::{DBError, DBResult, QueryError};
use crate::Expression;

pub const MAX_EXPR_DEPTH: usize = 100;

/// Trait for validating expressions in a single tree traversal.
pub trait ExprValidator {
    /// Validate a single expression node.
    fn validate(&mut self, expr: &Expression, depth: usize) -> DBResult<()>;

    /// Validate children of the current node. The default implementation
    /// iterates over `expr.children()` and calls `walk_expr` for each.
    fn validate_children(&mut self, expr: &Expression, depth: usize) -> DBResult<()> {
        for child in expr.children() {
            walk_expr(child, depth + 1, self)?;
        }
        Ok(())
    }
}

/// Walk the expression tree depth-first, calling the validator on each node.
pub fn walk_expr<V: ExprValidator + ?Sized>(
    expr: &Expression,
    depth: usize,
    validator: &mut V,
) -> DBResult<()> {
    if depth > MAX_EXPR_DEPTH {
        return Err(DBError::from(QueryError::invalid_query(
            "expressions are nested too deeply in levels",
        )));
    }
    validator.validate(expr, depth)?;
    validator.validate_children(expr, depth)?;
    Ok(())
}