//! Logical join nodes: InnerJoin, LeftJoin, RightJoin, CrossJoin, FullOuterJoin, SemiJoin.

use graphdb_core::types::expr::contextual::ContextualExpression;

use crate::define_logical_join_node;

define_logical_join_node! {
    pub struct LogicalInnerJoinNode {}
    enum: InnerJoin
}

define_logical_join_node! {
    pub struct LogicalLeftJoinNode {}
    enum: LeftJoin
}

define_logical_join_node! {
    pub struct LogicalRightJoinNode {}
    enum: RightJoin
}

define_logical_join_node! {
    pub struct LogicalCrossJoinNode {}
    enum: CrossJoin
}

define_logical_join_node! {
    pub struct LogicalFullOuterJoinNode {}
    enum: FullOuterJoin
}

define_logical_join_node! {
    pub struct LogicalSemiJoinNode {
        join_condition: Option<ContextualExpression>,
        anti: bool,
    }
    enum: SemiJoin
}
