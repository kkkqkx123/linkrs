//! Tag statistics module
//!
//! Provide tag-level statistical information for use in querying the estimates made by the optimization engine.

/// Tag statistics information
#[derive(Debug, Clone)]
pub struct TagStatistics {
    /// Tag name
    pub tag_name: String,
    /// Number of vertices
    pub vertex_count: u64,
    /// Average Outdegree (Key Metric: Impact on the Cost of Traversal)
    pub avg_out_degree: f64,
    /// Average Indegree
    pub avg_in_degree: f64,
}

impl TagStatistics {
    /// Create new tag statistics information.
    pub fn new(tag_name: String) -> Self {
        Self {
            tag_name,
            vertex_count: 0,
            avg_out_degree: 0.0,
            avg_in_degree: 0.0,
        }
    }
}

impl Default for TagStatistics {
    fn default() -> Self {
        Self::new(String::new())
    }
}
