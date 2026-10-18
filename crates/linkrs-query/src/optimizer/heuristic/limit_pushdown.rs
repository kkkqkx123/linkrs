//! LIMIT push-down optimization rule
//!
//! These rules are responsible for pushing the LIMIT operation down to the lowest level of the planning tree, in order to reduce the amount of data that needs to be processed.
//! Sort-to-TopN conversion lives solely in the cost path
//! (`cost_based::topn_wiring` + `SortEliminationOptimizer`); no heuristic
//! equivalent is registered so the decision source stays unique.

pub mod push_limit_down_get_edges;
pub mod push_limit_down_get_vertices;
pub mod push_limit_down_index_scan;
pub mod push_limit_down_scan_edges;
pub mod push_limit_down_scan_vertices;
pub mod push_topn_down_index_scan;

// Export all rules
pub use push_limit_down_get_edges::PushLimitDownGetEdgesRule;
pub use push_limit_down_get_vertices::PushLimitDownGetVerticesRule;
pub use push_limit_down_index_scan::PushLimitDownIndexScanRule;
pub use push_limit_down_scan_edges::PushLimitDownScanEdgesRule;
pub use push_limit_down_scan_vertices::PushLimitDownScanVerticesRule;
pub use push_topn_down_index_scan::PushTopNDownIndexScanRule;
