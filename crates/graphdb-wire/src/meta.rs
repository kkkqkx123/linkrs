//! Meta contract DTOs: auth, session, transaction, config, statistics and
//! cold-snapshot endpoints.
//!
//! The CLI-side mirrors (`cli/client/{config_types,stats,snapshot,
//! transaction}.rs` and parts of `request_types.rs`) are consolidated here.

use serde::{Deserialize, Serialize};

// ── Auth ──────────────────────────────────────────────────────────────────

/// Login request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LoginResponse {
    pub session_id: i64,
    pub username: String,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_role: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

/// Current user response for role-aware clients.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuthMeResponse {
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_role: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

/// User list entry for the management view.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UserListItem {
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active: Option<String>,
}

/// User list response for the management view.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UserListResponse {
    #[serde(default)]
    pub users: Vec<UserListItem>,
}

/// Logout request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LogoutRequest {
    pub session_id: i64,
}

// ── Session ───────────────────────────────────────────────────────────────

/// Create session request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateSessionRequest {
    pub username: String,
    pub client_ip: String,
}

/// Session response
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionResponse {
    pub session_id: i64,
    pub username: String,
    #[serde(default)]
    pub created_at: u64,
}

// ── Transaction ───────────────────────────────────────────────────────────

/// Begin transaction request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BeginTransactionRequest {
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub query_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub statement_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub idle_timeout_seconds: Option<u64>,
    /// `repeatable_read` or `read_committed`.
    #[serde(default)]
    pub isolation_level: Option<String>,
}

/// Transaction response
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionResponse {
    pub transaction_id: u64,
    pub status: String,
}

/// Transaction action request (commit/rollback body).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionActionRequest {
    pub session_id: i64,
}

// ── Config ────────────────────────────────────────────────────────────────

/// Update config request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UpdateConfigRequest {
    pub section: String,
    pub key: String,
    pub value: serde_json::Value,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServerConfig {
    pub version: String,
    #[serde(default)]
    pub sections: Vec<ConfigSection>,
}

/// Configuration section
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigSection {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub items: Vec<ConfigItem>,
}

/// Configuration item
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigItem {
    pub key: String,
    pub value: serde_json::Value,
    #[serde(default)]
    pub default_value: Option<serde_json::Value>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mutable: bool,
}

// ── Statistics ────────────────────────────────────────────────────────────

/// Statistics for a session
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionStatistics {
    pub total_queries: u64,
    pub total_changes: u64,
    pub avg_execution_time_ms: f64,
}

/// Query type statistics
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryStatistics {
    pub total_queries: u64,
    #[serde(default)]
    pub slow_queries: Vec<SlowQueryInfo>,
    pub query_types: QueryTypeStatistics,
}

/// Information about a slow query
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SlowQueryInfo {
    pub trace_id: String,
    pub session_id: i64,
    pub query: String,
    pub duration_ms: f64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<crate::query::QueryStageTimings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_node_count: Option<usize>,
}

/// Database statistics
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DatabaseStatistics {
    pub space_count: i64,
    pub total_vertices: i64,
    pub total_edges: i64,
    pub total_queries: u64,
    pub active_queries: u64,
    pub queries_per_second: f64,
    pub avg_latency_ms: f64,
}

/// Memory usage block shared by system responses.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MemoryUsage {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

/// Connection counters shared by system responses.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConnectionStats {
    pub active: usize,
    pub total: usize,
    pub max: usize,
}

/// Latency percentiles in microseconds.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LatencyPercentilesUs {
    pub avg: u64,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
}

/// System resource response with stable keys plus process and disk detail.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SystemResourceResponse {
    pub cpu_usage_percent: f64,
    pub memory_usage: MemoryUsage,
    pub connections: ConnectionStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_dir_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_descriptors: Option<u64>,
}

/// Checkpoint summary embedded in database storage sections.
#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct CheckpointSummary {
    #[serde(default)]
    pub success_count: u64,
    #[serde(default)]
    pub failure_count: u64,
    #[serde(default)]
    pub trigger_count: u64,
    #[serde(default)]
    pub avg_duration_us: f64,
    #[serde(default)]
    pub triggered_by_wal_size: u64,
    #[serde(default)]
    pub triggered_by_interval: u64,
    #[serde(default)]
    pub triggered_explicit: u64,
}

/// Database overview response mirroring the handler JSON keys.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DatabaseOverviewResponse {
    pub spaces: DatabaseSpaces,
    pub storage: DatabaseStorage,
    pub performance: DatabasePerformance,
    pub search: DatabaseSearchSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DatabaseSpaces {
    pub count: usize,
    pub total_vertices: usize,
    pub total_edges: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DatabaseStorage {
    pub total_size_bytes: u64,
    pub index_size_bytes: u64,
    pub data_size_bytes: u64,
    #[serde(default)]
    pub fragmentation_permille: u64,
    #[serde(default)]
    pub wasted_bytes: u64,
    #[serde(default)]
    pub tombstone_count: u64,
    #[serde(default)]
    pub tombstone_memory_bytes: u64,
    #[serde(default)]
    pub dirty_pages: u64,
    #[serde(default)]
    pub dirty_pages_total: u64,
    #[serde(default)]
    pub checkpoint: CheckpointSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DatabasePerformance {
    pub total_queries: u64,
    pub active_queries: u64,
    pub query_cache_size: usize,
    pub queries_per_second: f64,
    pub avg_latency_ms: f64,
    pub cache_hit_rate: f64,
    #[serde(default)]
    pub cache_hit_rate_source: String,
    #[serde(default)]
    pub error_total: u64,
    #[serde(default)]
    pub latency_percentiles_us: Option<LatencyPercentilesUs>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DatabaseSearchSummary {
    pub total_queries: u64,
    pub total_errors: u64,
    pub avg_latency_ms: f64,
    pub total_index_operations: u64,
    pub total_delete_operations: u64,
    pub cache_hit_count: u64,
    pub cache_miss_count: u64,
    pub cache_hit_rate: f64,
}

/// Aggregated query pattern entry.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryPatternEntry {
    pub normalized_query: String,
    pub query_type: String,
    #[serde(default)]
    pub labels: Vec<String>,
    pub execution_count: u64,
    pub avg_duration_ms: f64,
    pub p95_duration_ms: f64,
    pub p99_duration_ms: f64,
    pub error_rate: f64,
    #[serde(default)]
    pub error_count: u64,
}

/// Executor rollup entry.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExecutorSummaryEntry {
    pub executor_type: String,
    pub count: u64,
    pub total_time_ms: u64,
    pub total_rows: u64,
    pub avg_time_ms: f64,
}

/// Query statistics response with time-window filtering and error breakdowns.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryStatsResponse {
    pub total_queries: u64,
    pub slow_queries: Vec<SlowQueryInfo>,
    pub query_types: QueryTypeStatistics,
    #[serde(default)]
    pub errors_by_type: std::collections::HashMap<String, u64>,
    #[serde(default)]
    pub errors_by_phase: std::collections::HashMap<String, u64>,
    #[serde(default)]
    pub error_total: u64,
    #[serde(default)]
    pub top_patterns: Vec<QueryPatternEntry>,
    #[serde(default)]
    pub executor_summary: Vec<ExecutorSummaryEntry>,
    #[serde(default)]
    pub latency_percentiles_us: Option<LatencyPercentilesUs>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

/// Per-index search breakdown entry.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchIndexEntry {
    pub index: String,
    pub search_queries: u64,
    pub search_errors: u64,
    pub search_latency_ms: u64,
    pub avg_search_latency_ms: f64,
    pub index_operations: u64,
    pub index_errors: u64,
    pub index_latency_ms: u64,
}

/// Search statistics response with per-index detail.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchStatsResponse {
    pub search: SearchSection,
    pub index: IndexSection,
    pub delete: DeleteSection,
    pub cache: CacheSection,
    #[serde(default)]
    pub by_index: Vec<SearchIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchSection {
    pub total_queries: u64,
    pub total_errors: u64,
    pub total_latency_ms: u64,
    pub avg_latency_ms: f64,
    pub total_results: u64,
    pub latency_percentiles_us: LatencyPercentilesUs,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IndexSection {
    pub total_operations: u64,
    pub total_errors: u64,
    pub total_latency_ms: u64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeleteSection {
    pub total_operations: u64,
    pub total_errors: u64,
    pub total_latency_ms: u64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CacheSection {
    pub hit_count: u64,
    pub miss_count: u64,
    pub hit_rate: f64,
}

/// Cross-subsystem overview for the monitoring first screen.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OverviewResponse {
    pub system: SystemResourceResponse,
    pub database: DatabaseOverviewResponse,
    pub query_latency_us: LatencyPercentilesUs,
    pub errors: OverviewErrors,
    pub storage: OverviewStorage,
    pub transaction: OverviewTransaction,
    pub sync: OverviewSync,
    pub timeseries: Vec<OverviewTimeseriesPoint>,
}

/// Sync summary embedded in the overview; mirrors the sync status shape.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OverviewSync {
    pub is_running: bool,
    pub outbox_pending: usize,
    pub outbox_retries: u64,
    pub outbox_dead_lettered: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OverviewErrors {
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OverviewStorage {
    pub read_ops: u64,
    pub write_ops: u64,
    pub fragmentation_permille: u64,
    pub tombstone_count: u64,
    pub checkpoint_success: u64,
    pub checkpoint_failure: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OverviewTransaction {
    pub begun: u64,
    pub committed: u64,
    pub rolled_back: u64,
    pub active: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OverviewTimeseriesPoint {
    pub second: u64,
    pub queries: u64,
    pub avg_latency_ms: f64,
    pub errors: u64,
}

/// Single query portrait detail for console jumps.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryProfileDetailResponse {
    pub trace_id: String,
    pub session_id: i64,
    pub query: String,
    pub duration_ms: f64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<crate::query::QueryStageTimings>,
    #[serde(default)]
    pub executors: Vec<QueryProfileExecutor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_node_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryProfileExecutor {
    pub executor_type: String,
    pub duration_ms: f64,
    pub rows: usize,
    pub memory_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_roundtrip() {
        let request = LoginRequest {
            username: "root".to_string(),
            password: "secret".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: LoginRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.username, "root");
    }

    #[test]
    fn transaction_response_roundtrip() {
        let response = TransactionResponse {
            transaction_id: 42,
            status: "Active".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let back: TransactionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.transaction_id, 42);
        assert_eq!(back.status, "Active");
    }

    #[test]
    fn begin_transaction_request_defaults() {
        let back: BeginTransactionRequest = serde_json::from_str(r#"{"read_only": true}"#).unwrap();
        assert!(back.read_only);
        assert!(back.timeout_seconds.is_none());
        assert!(back.isolation_level.is_none());
    }

    #[test]
    fn server_config_roundtrip() {
        let config = ServerConfig {
            version: "0.1.0".to_string(),
            sections: vec![ConfigSection {
                name: "database".to_string(),
                description: None,
                items: vec![ConfigItem {
                    key: "host".to_string(),
                    value: serde_json::json!("127.0.0.1"),
                    default_value: None,
                    description: None,
                    mutable: true,
                }],
            }],
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, "0.1.0");
        assert_eq!(back.sections[0].items[0].key, "host");
    }

    #[test]
    fn query_statistics_roundtrip() {
        let stats = QueryStatistics {
            total_queries: 10,
            slow_queries: Vec::new(),
            query_types: QueryTypeStatistics {
                match_queries: 4,
                create_queries: 0,
                update_queries: 0,
                delete_queries: 0,
                insert_queries: 6,
                go_queries: 0,
                fetch_queries: 0,
                lookup_queries: 0,
                show_queries: 0,
            },
        };
        let json = serde_json::to_string(&stats).unwrap();
        let back: QueryStatistics = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_queries, 10);
        assert_eq!(back.query_types.match_queries, 4);
    }

    #[test]
    fn update_config_request_roundtrip() {
        let request = UpdateConfigRequest {
            section: "database".to_string(),
            key: "port".to_string(),
            value: serde_json::json!(9090),
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: UpdateConfigRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.section, "database");
        assert_eq!(back.key, "port");
        assert_eq!(back.value, serde_json::json!(9090));
    }
}
