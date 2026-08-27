//! Projection Downward Optimization Rules
//!
//! These rules push projection operations towards the data source, reducing
//! the amount of data that needs to be transmitted.
//!
//! Scan-level and graph-operator-level rules are enabled. The graph-operator
//! rules (`GetVertices` / `GetNeighbors` / `GetEdges`) narrow
//! `projected_properties` through the typed [`RequiredPropertyAnalyzer`],
//! which only prunes provable `var.prop` references and keeps the `Project`
//! node intact.

pub mod push_project_down_append_vertices;
pub mod push_project_down_get_edges;
pub mod push_project_down_get_neighbors;
pub mod push_project_down_get_vertices;
pub mod push_project_down_scan_edges;
pub mod push_project_down_scan_vertices;

pub use push_project_down_append_vertices::PushProjectDownAppendVerticesRule;
pub use push_project_down_get_edges::PushProjectDownGetEdgesRule;
pub use push_project_down_get_neighbors::PushProjectDownGetNeighborsRule;
pub use push_project_down_get_vertices::PushProjectDownGetVerticesRule;
pub use push_project_down_scan_edges::PushProjectDownScanEdgesRule;
pub use push_project_down_scan_vertices::PushProjectDownScanVerticesRule;
