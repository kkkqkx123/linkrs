use crate::query::executor::base::EdgeDirection;

/// General characteristics of graph traversal executors
///
/// This trait provides a unified configuration interface for graph traversal executors.
/// All graph traversal executors should implement this trait to provide consistent configuration management.
pub trait GraphTraversalExecutor<S> {
    /// Set the border direction
    fn set_edge_direction(&mut self, direction: EdgeDirection);

    /// Set edge type filtering
    fn set_edge_types(&mut self, edge_types: Option<Vec<String>>);

    /// Set the maximum depth.
    fn set_max_depth(&mut self, max_depth: Option<usize>);

    /// Get the current edge direction.
    fn get_edge_direction(&self) -> EdgeDirection;

    /// Get the current edge type filter.
    fn get_edge_types(&self) -> Option<Vec<String>>;

    /// Obtain the current maximum depth.
    fn get_max_depth(&self) -> Option<usize>;

    /// Verify whether the executor configuration is valid.
    fn validate_config(&self) -> Result<(), String>;

    /// Obtain executor statistics information.
    fn get_stats(&self) -> TraversalStats;
}

/// Graph traversal statistics
#[derive(Debug, Clone, Default)]
pub struct TraversalStats {
    pub nodes_visited: usize,
    pub edges_traversed: usize,
    pub execution_time_ms: u64,
    pub max_depth_reached: usize,
}
