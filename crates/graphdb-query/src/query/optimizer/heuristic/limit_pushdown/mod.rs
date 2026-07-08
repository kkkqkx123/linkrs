//! LIMIT push-down optimization rule
//!
//! These rules are responsible for pushing the LIMIT operation down to the lowest level of the planning tree, in order to reduce the amount of data that needs to be processed.

pub mod convert_sort_limit_to_topn;
pub mod push_limit_down_get_edges;
pub mod push_limit_down_get_vertices;
pub mod push_limit_down_index_scan;
pub mod push_limit_down_scan_edges;
pub mod push_limit_down_scan_vertices;
pub mod push_topn_down_index_scan;

// Export all rules
pub use convert_sort_limit_to_topn::ConvertSortLimitToTopNRule;
pub use push_limit_down_get_edges::PushLimitDownGetEdgesRule;
pub use push_limit_down_get_vertices::PushLimitDownGetVerticesRule;
pub use push_limit_down_index_scan::PushLimitDownIndexScanRule;
pub use push_limit_down_scan_edges::PushLimitDownScanEdgesRule;
pub use push_limit_down_scan_vertices::PushLimitDownScanVerticesRule;
pub use push_topn_down_index_scan::PushTopNDownIndexScanRule;
