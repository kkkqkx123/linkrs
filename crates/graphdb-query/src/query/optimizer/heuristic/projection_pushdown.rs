//! Projection Downward Optimization Rules
//!
//! These rules push projection operations towards the data source, reducing
//! the amount of data that needs to be transmitted.
//!
//! Only scan-level rules are enabled. Get-level rules are
//! deferred to Phase 4 (typed required-property pruning).

pub mod push_project_down_scan_edges;
pub mod push_project_down_scan_vertices;

pub use push_project_down_scan_edges::PushProjectDownScanEdgesRule;
pub use push_project_down_scan_vertices::PushProjectDownScanVerticesRule;
