use super::expr::BoundExpression;

use crate::parser::ast::{LimitClause, SampleClause, SkipClause};
use linkrs_core::types::OrderDirection;

/// A unified projection item used by RETURN, YIELD, WITH, and COLLECT clauses.
///
/// Previously `BoundReturnItem` and `BoundYieldItem` existed as identical
/// separate structs; they have been merged into this single type.
#[derive(Debug, Clone)]
pub struct BoundProjectionItem {
    pub expression: BoundExpression,
    pub alias: Option<String>,
}

/// Backward-compatible alias for [`BoundProjectionItem`].
pub type BoundReturnItem = BoundProjectionItem;
/// Backward-compatible alias for [`BoundProjectionItem`].
pub type BoundYieldItem = BoundProjectionItem;

#[derive(Debug, Clone)]
pub struct BoundOrderByItem {
    pub expression: BoundExpression,
    pub direction: OrderDirection,
}

#[derive(Debug, Clone)]
pub struct BoundReturnClause {
    pub items: Vec<BoundProjectionItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
    pub sample: Option<SampleClause>,
}

#[derive(Debug, Clone)]
pub struct BoundYieldClause {
    pub items: Vec<BoundProjectionItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub limit: Option<LimitClause>,
    pub skip: Option<SkipClause>,
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
