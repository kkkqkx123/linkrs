//! Session-level change statistics module
//!
//! Provides statistics such as the number of rows affected by query execution, last insertion ID, etc.

pub use crate::core::SessionStatistics;

/// Statistical information on query results
///
/// Statistics extracted from query results
#[derive(Debug, Clone, Default)]
pub struct QueryStatistics {
    /// Number of rows affected
    pub rows_affected: u64,
    /// Number of rows returned
    pub rows_returned: u64,
    /// List of inserted vertex IDs
    pub inserted_vertex_ids: Vec<u64>,
    /// List of inserted edge IDs
    pub inserted_edge_ids: Vec<u64>,
    /// Number of vertices updated
    pub vertices_updated: u64,
    /// Updated number of edges
    pub edges_updated: u64,
    /// Number of vertices deleted
    pub vertices_deleted: u64,
    /// Number of edges deleted
    pub edges_deleted: u64,
}

impl QueryStatistics {
    /// Creating empty statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Created from query result metadata
    ///
    /// # Parameters
    /// - `metadata` - query result metadata
    pub fn from_metadata(metadata: &crate::api::core::ExecutionMetadata) -> Self {
        Self {
            rows_affected: metadata.rows_returned,
            rows_returned: metadata.rows_returned,
            ..Default::default()
        }
    }

    /// Merging another statistic
    pub fn merge(&mut self, other: &QueryStatistics) {
        self.rows_affected += other.rows_affected;
        self.rows_returned += other.rows_returned;
        self.inserted_vertex_ids
            .extend_from_slice(&other.inserted_vertex_ids);
        self.inserted_edge_ids
            .extend_from_slice(&other.inserted_edge_ids);
        self.vertices_updated += other.vertices_updated;
        self.edges_updated += other.edges_updated;
        self.vertices_deleted += other.vertices_deleted;
        self.edges_deleted += other.edges_deleted;
    }

    /// Getting the total number of changes
    pub fn total_changes(&self) -> u64 {
        self.rows_affected
            + self.vertices_updated
            + self.edges_updated
            + self.vertices_deleted
            + self.edges_deleted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_statistics_basic() {
        let stats = SessionStatistics::new();

        assert_eq!(stats.last_changes(), 0);
        assert_eq!(stats.total_changes(), 0);
        assert_eq!(stats.last_insert_vertex_id(), None);
        assert_eq!(stats.last_insert_edge_id(), None);
    }

    #[test]
    fn test_record_changes() {
        let stats = SessionStatistics::new();

        stats.record_changes(5);
        assert_eq!(stats.last_changes(), 5);
        assert_eq!(stats.total_changes(), 5);

        stats.record_changes(3);
        assert_eq!(stats.last_changes(), 3);
        assert_eq!(stats.total_changes(), 8);
    }

    #[test]
    fn test_record_vertex_insert() {
        let stats = SessionStatistics::new();

        stats.record_vertex_insert(100);
        assert_eq!(stats.last_insert_vertex_id(), Some(100));
        assert_eq!(stats.last_changes(), 1);
        assert_eq!(stats.total_changes(), 1);

        // Invalid IDs should not be logged
        stats.record_vertex_insert(0);
        assert_eq!(stats.last_insert_vertex_id(), Some(100)); // Of course! Please provide the text you would like to have translated.
    }

    #[test]
    fn test_record_edge_insert() {
        let stats = SessionStatistics::new();

        stats.record_edge_insert(200);
        assert_eq!(stats.last_insert_edge_id(), Some(200));
        assert_eq!(stats.last_changes(), 1);
        assert_eq!(stats.total_changes(), 1);
    }

    #[test]
    fn test_reset() {
        let stats = SessionStatistics::new();

        stats.record_changes(5);
        stats.record_vertex_insert(100);

        stats.reset_last();
        assert_eq!(stats.last_changes(), 0);
        assert_eq!(stats.last_insert_vertex_id(), None);
        assert_eq!(stats.total_changes(), 6); // The total amount remains unchanged.

        stats.reset_all();
        assert_eq!(stats.total_changes(), 0);
    }

    #[test]
    fn test_query_statistics() {
        let mut stats = QueryStatistics::new();
        stats.rows_affected = 10;
        stats.rows_returned = 5;
        stats.inserted_vertex_ids = vec![1, 2, 3];

        assert_eq!(stats.total_changes(), 10);

        let mut other = QueryStatistics::new();
        other.rows_affected = 5;
        other.inserted_vertex_ids = vec![4, 5];

        stats.merge(&other);
        assert_eq!(stats.rows_affected, 15);
        assert_eq!(stats.inserted_vertex_ids.len(), 5);
    }
}
