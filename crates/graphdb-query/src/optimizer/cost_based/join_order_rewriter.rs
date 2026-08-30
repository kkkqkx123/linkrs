mod types;
mod flatten;
mod reorder;

pub use types::{LeafInfo, JoinPredicate, FlattenedJoinChain, LeafInfoLogical, FlattenedJoinChainLogical};
pub(crate) use reorder::{
    walk_and_optimize_joins, walk_and_optimize_joins_with_decisions, walk_and_optimize_joins_logical,
};
