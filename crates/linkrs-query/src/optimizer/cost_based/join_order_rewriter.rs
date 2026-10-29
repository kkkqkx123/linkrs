mod flatten;
mod reorder;
mod types;

pub(crate) use reorder::{walk_and_optimize_joins, walk_and_optimize_joins_logical};
pub use types::{
    FlattenedJoinChain, FlattenedJoinChainLogical, JoinPredicate, LeafInfo, LeafInfoLogical,
};
