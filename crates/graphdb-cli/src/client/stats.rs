//! Statistics types

use serde::Deserialize;

/// Statistics for a session
#[derive(Debug, Clone, Deserialize)]
pub struct SessionStatistics {
    pub total_queries: u64,
    pub total_changes: u64,
    pub avg_execution_time_ms: f64,
}

/// Query type statistics
#[derive(Debug, Clone, Deserialize)]
pub struct QueryTypeStatistics {
    pub match_queries: u64,
    pub create_queries: u64,
    pub update_queries: u64,
    pub delete_queries: u64,
    pub insert_queries: u64,
    pub go_queries: u64,
    pub fetch_queries: u64,
    pub lookup_queries: u64,
    pub show_queries: u64,
}

/// Query statistics
#[derive(Debug, Clone, Deserialize)]
pub struct QueryStatistics {
    pub total_queries: u64,
    pub slow_queries: Vec<SlowQueryInfo>,
    pub query_types: QueryTypeStatistics,
}

/// Information about a slow query
#[derive(Debug, Clone, Deserialize)]
pub struct SlowQueryInfo {
    pub trace_id: String,
    pub session_id: i64,
    pub query: String,
    pub duration_ms: f64,
    pub status: String,
}

/// Database statistics
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseStatistics {
    pub space_count: i64,
    pub total_vertices: i64,
    pub total_edges: i64,
    pub total_queries: u64,
    pub active_queries: u64,
    pub queries_per_second: f64,
    pub avg_latency_ms: f64,
}
