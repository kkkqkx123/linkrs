//! Projection Downward Optimization Rules
//!
//! These rules are responsible for pushing the projection operations towards the data source, thereby reducing the amount of data that needs to be transmitted.

pub mod push_project_down_edge_index_scan;
pub mod push_project_down_get_edges;
pub mod push_project_down_get_neighbors;
pub mod push_project_down_get_vertices;
pub mod push_project_down_scan_edges;
pub mod push_project_down_scan_vertices;

pub use push_project_down_edge_index_scan::PushProjectDownEdgeIndexScanRule;
pub use push_project_down_get_edges::PushProjectDownGetEdgesRule;
pub use push_project_down_get_neighbors::PushProjectDownGetNeighborsRule;
pub use push_project_down_get_vertices::PushProjectDownGetVerticesRule;
pub use push_project_down_scan_edges::PushProjectDownScanEdgesRule;
pub use push_project_down_scan_vertices::PushProjectDownScanVerticesRule;
