//! Immutable configuration for binary join operators.

use graphdb_core::types::expr::Expression;

/// Which physical child of a hash join provides the build side.
///
/// The logical plan always builds from the right child (the default); a left
/// build side is a physical alternative selected by the plan conversion when
/// the right child is not hashable but the left child is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuildSide {
    Left,
    #[default]
    Right,
}

/// Immutable config for binary join operators.
#[derive(Debug, Clone)]
pub enum JoinSpec {
    InnerJoin {
        join_condition: Option<Expression>,
    },
    LeftJoin {
        join_condition: Option<Expression>,
    },
    RightJoin {
        join_condition: Option<Expression>,
    },
    FullOuterJoin {
        join_condition: Option<Expression>,
    },
    CrossJoin,
    SemiJoin {
        join_condition: Option<Expression>,
        // Whether this is a NOT EXISTS (anti) semi join: keep left rows
        // with NO matching right row.
        anti: bool,
    },
    HashJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side: BuildSide,
    },
    HashLeftJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side: BuildSide,
    },
    NestedLoopJoin {
        join_condition: Option<Expression>,
    },
}
