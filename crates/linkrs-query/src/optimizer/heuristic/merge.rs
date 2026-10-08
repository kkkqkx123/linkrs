//! Operation to merge and optimize rules
//!
//! These rules are responsible for merging multiple consecutive operations of the same type in order to reduce the number of intermediate results and the computational overhead associated with their execution.

pub mod collapse_consecutive_project;
pub mod collapse_project;
pub mod combine_filter;
pub mod merge_get_nbrs_and_dedup;
pub mod merge_get_nbrs_and_project;
pub mod merge_get_vertices_and_dedup;
pub mod merge_get_vertices_and_project;

// Export all rules
pub use collapse_consecutive_project::CollapseConsecutiveProjectRule;
pub use collapse_project::CollapseProjectRule;
pub use combine_filter::CombineFilterRule;
pub use merge_get_nbrs_and_dedup::MergeGetNbrsAndDedupRule;
pub use merge_get_nbrs_and_project::MergeGetNbrsAndProjectRule;
pub use merge_get_vertices_and_dedup::MergeGetVerticesAndDedupRule;
pub use merge_get_vertices_and_project::MergeGetVerticesAndProjectRule;
