//! Remove the optimization rules.
//!
//! These rules are responsible for eliminating redundant operations, such as filtering tautologies, performing no-operation projections, and removing unnecessary duplicates.

pub mod dedup_elimination;
pub mod eliminate_append_vertices;
pub mod eliminate_empty_set_operation;
pub mod eliminate_filter;
pub mod eliminate_row_collect;
pub mod eliminate_sort;
pub mod remove_append_vertices_below_join;
pub mod remove_noop_project;

// Export all rules
pub use dedup_elimination::DedupEliminationRule;
pub use eliminate_append_vertices::EliminateAppendVerticesRule;
pub use eliminate_empty_set_operation::EliminateEmptySetOperationRule;
pub use eliminate_filter::EliminateFilterRule;
pub use eliminate_row_collect::EliminateRowCollectRule;
pub use eliminate_sort::EliminateSortRule;
pub use remove_append_vertices_below_join::RemoveAppendVerticesBelowJoinRule;
pub use remove_noop_project::RemoveNoopProjectRule;
