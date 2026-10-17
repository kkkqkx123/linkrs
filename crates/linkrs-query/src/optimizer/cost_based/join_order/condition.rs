//! Join condition description used by the join order optimizer.

use crate::optimizer::cost_based::ndv::DEFAULT_JOIN_SELECTIVITY;
use linkrs_core::types::ContextualExpression;

/// Connection conditions
#[derive(Debug, Clone)]
pub struct JoinCondition {
    /// Left table ID
    pub left_table: String,
    /// Right table ID
    pub right_table: String,
    /// Connection selectivity (estimated proportion of successful connection outcomes)
    pub selectivity: f64,
    /// Connection expression
    pub expression: Option<ContextualExpression>,
}

impl JoinCondition {
    /// Create new connection conditions.
    pub fn new(left_table: String, right_table: String) -> Self {
        Self {
            left_table,
            right_table,
            selectivity: DEFAULT_JOIN_SELECTIVITY,
            expression: None,
        }
    }

    /// Set the option to be selective.
    pub fn with_selectivity(mut self, selectivity: f64) -> Self {
        self.selectivity = selectivity.clamp(0.0, 1.0);
        self
    }

    /// Setting the connection expression
    pub fn with_expression(mut self, expression: ContextualExpression) -> Self {
        self.expression = Some(expression);
        self
    }
}
