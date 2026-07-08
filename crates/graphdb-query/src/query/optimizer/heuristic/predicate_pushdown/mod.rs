//! Predicate Pushdown Optimization Rule
//!
//! These rules are responsible for pushing the filtering conditions down to the lowest levels of the planning tree, in order to reduce the amount of data that needs to be processed.

pub mod push_efilter_down;
pub mod push_filter_down_all_paths;
pub mod push_filter_down_cross_join;
pub mod push_filter_down_expand_all;
pub mod push_filter_down_get_nbrs;
pub mod push_filter_down_hash_inner_join;
pub mod push_filter_down_hash_left_join;
pub mod push_filter_down_inner_join;
pub mod push_filter_down_node;
pub mod push_filter_down_traverse;
pub mod push_vfilter_down_scan_vertices;

pub use push_efilter_down::PushEFilterDownRule;
pub use push_filter_down_all_paths::PushFilterDownAllPathsRule;
pub use push_filter_down_cross_join::PushFilterDownCrossJoinRule;
pub use push_filter_down_expand_all::PushFilterDownExpandAllRule;
pub use push_filter_down_get_nbrs::PushFilterDownGetNbrsRule;
pub use push_filter_down_hash_inner_join::PushFilterDownHashInnerJoinRule;
pub use push_filter_down_hash_left_join::PushFilterDownHashLeftJoinRule;
pub use push_filter_down_inner_join::PushFilterDownInnerJoinRule;
pub use push_filter_down_node::PushFilterDownNodeRule;
pub use push_filter_down_traverse::PushFilterDownTraverseRule;
pub use push_vfilter_down_scan_vertices::PushVFilterDownScanVerticesRule;
