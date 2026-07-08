//! JOIN Optimization Rules
//!
//! This module contains optimization rules specifically for JOIN operations.
//! These rules aim to improve JOIN performance through various transformations.
//!
//! # Phase 1: Basic JOIN Optimization
//! - PushProjectDownJoinRule: Push projection operations down to JOIN inputs
//! - LeftJoinToInnerJoinRule: Convert LeftJoin to InnerJoin when possible
//! - JoinConditionSimplifyRule: Simplify and deduplicate JOIN conditions
//!
//! # Phase 2: Graph Traversal Optimization
//! - JoinToExpandRule: Convert vertex-edge JOIN to ExpandAll
//! - JoinToAppendVerticesRule: Convert edge-vertex JOIN to AppendVertices
//! - MergeConsecutiveExpandRule: Merge consecutive ExpandAll into Traverse
//!
//! # Phase 3: Advanced Optimization
//! - JoinEliminationRule: Eliminate redundant JOIN operations
//! - IndexJoinSelectionRule: Select index-based JOIN when appropriate
//! - JoinReorderRule: Reorder multi-table JOINs for better performance

pub mod index_join_selection;
pub mod join_condition_simplify;
pub mod join_elimination;
pub mod join_reorder;
pub mod join_to_append_vertices;
pub mod join_to_expand;
pub mod left_join_to_inner_join;
pub mod merge_consecutive_expand;
pub mod push_project_down_join;

pub use index_join_selection::IndexJoinSelectionRule;
pub use join_condition_simplify::JoinConditionSimplifyRule;
pub use join_elimination::JoinEliminationRule;
pub use join_reorder::JoinReorderRule;
pub use join_to_append_vertices::JoinToAppendVerticesRule;
pub use join_to_expand::JoinToExpandRule;
pub use left_join_to_inner_join::LeftJoinToInnerJoinRule;
pub use merge_consecutive_expand::MergeConsecutiveExpandRule;
pub use push_project_down_join::PushProjectDownJoinRule;
