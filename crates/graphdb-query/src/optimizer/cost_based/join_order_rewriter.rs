mod flatten;
mod reorder;
mod types;

pub(crate) use reorder::{walk_and_optimize_joins_logical, walk_and_optimize_joins_with_decisions};
pub use types::{
    FlattenedJoinChain, FlattenedJoinChainLogical, JoinPredicate, LeafInfo, LeafInfoLogical,
};
