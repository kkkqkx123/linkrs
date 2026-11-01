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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub must_change_password: Option<bool>,
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

/// Create user request for the admin channel.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
}

/// Reset password request for the admin channel.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResetPasswordRequest {
    pub password: String,
}

/// Grant role request for the admin channel.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GrantRoleRequest {
    pub role: String,
    pub space: String,
}

/// Revoke role request for the admin channel.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RevokeRoleRequest {
    pub space: String,
}

// ── Session ───────────────────────────────────────────────────────────────

/// Create session request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateSessionRequest {
    pub username: String,
    pub client_ip: String,
    /// Optional password. Required when authentication is enabled;
    /// omitted or left empty when the server has disabled auth.
    #[serde(default)]
    pub password: Option<String>,
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

/// Create savepoint request for an explicit server-side transaction.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateSavepointRequest {
    #[serde(default)]
    pub name: Option<String>,
}

/// Savepoint descriptor returned by the server.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SavepointResponse {
    pub savepoint_id: u64,
    pub transaction_id: u64,
    #[serde(default)]
    pub name: Option<String>,
}

// ── Config ────────────────────────────────────────────────────────────────

/// Update config request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UpdateConfigRequest {
    pub section: String,
    pub key: String,
    #[schema(value_type = crate::query::CellValue)]
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
    #[schema(value_type = crate::query::CellValue)]
    pub value: serde_json::Value,
    #[serde(default)]
    #[schema(value_type = Option<crate::query::CellValue>)]
    pub default_value: Option<serde_json::Value>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mutable: bool,
}

/// Database section of the configuration snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DatabaseConfigSnapshot {
    pub host: String,
    pub port: u16,
    pub storage_path: String,
    pub max_sessions: usize,
}

/// Transaction section of the configuration snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionConfigSnapshot {
    pub default_timeout: u64,
    pub max_concurrent_transactions: usize,
    pub auto_commit: bool,
}

/// Log section of the configuration snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LogConfigSnapshot {
    pub level: String,
    pub dir: String,
    pub basename: String,
    pub max_file_size_mb: u64,
    pub max_files: usize,
    pub stdout: bool,
    pub json_format: bool,
}

/// Auth section of the configuration snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuthConfigSnapshot {
    pub enable_authorize: bool,
    pub failed_login_attempts: u32,
    pub session_idle_timeout_secs: u64,
    pub force_change_default_password: bool,
    pub default_username: String,
    pub bcrypt_cost: u32,
}

/// Bootstrap section of the configuration snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BootstrapConfigSnapshot {
    pub auto_create_default_space: bool,
    pub default_space_name: String,
    pub single_user_mode: bool,
}

/// Optimizer section of the configuration snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OptimizerConfigSnapshot {
    pub max_iteration_rounds: usize,
    pub max_exploration_rounds: usize,
    pub enable_cost_model: bool,
    pub enable_multi_plan: bool,
    pub enable_property_pruning: bool,
    pub enable_adaptive_iteration: bool,
    pub stable_threshold: usize,
    pub min_iteration_rounds: usize,
    pub statistics_sample_limit: usize,
    pub statistics_min_epoch_delta: u64,
    pub storage_cost_profile: String,
    #[serde(default)]
    pub space_cost_profiles: std::collections::HashMap<String, String>,
}

/// Monitoring section of the configuration snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MonitoringConfigSnapshot {
    pub enabled: bool,
    pub memory_cache_size: usize,
    pub slow_query_threshold_ms: u64,
}

/// Full configuration snapshot served by the management endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigSnapshot {
    pub database: DatabaseConfigSnapshot,
    pub transaction: TransactionConfigSnapshot,
    pub log: LogConfigSnapshot,
    pub auth: AuthConfigSnapshot,
    pub bootstrap: BootstrapConfigSnapshot,
    pub optimizer: OptimizerConfigSnapshot,
    pub monitoring: MonitoringConfigSnapshot,
}

/// Bulk configuration update receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigUpdateReceipt {
    #[serde(default)]
    pub updated: Vec<String>,
    #[serde(default)]
    pub requires_restart: Vec<String>,
    pub persisted: bool,
    pub message: String,
}

/// Single configuration item payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigItemResponse {
    pub section: String,
    pub key: String,
    #[schema(value_type = crate::query::CellValue)]
    pub value: serde_json::Value,
}

/// Single configuration item write receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfigItemWriteReceipt {
    pub section: String,
    pub key: String,
    #[schema(value_type = crate::query::CellValue)]
    pub value: serde_json::Value,
    pub requires_restart: bool,
    pub persisted: bool,
    pub message: String,
}

/// Dependency checks embedded in health responses.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthChecks {
    pub process: String,
    pub storage: String,
    pub transaction_manager: String,
    pub listeners: String,
}

/// Optional component markers embedded in health responses.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthOptional {
    pub vector: String,
    pub fulltext: String,
}

/// Service health payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
    pub ready: bool,
    pub live: bool,
    pub checks: HealthChecks,
    pub optional: HealthOptional,
}

/// Liveness probe payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LiveResponse {
    pub status: String,
    pub service: String,
    pub version: String,
    pub live: bool,
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
    /// Vector engine queries and writes. Scalar-only deployments report zeros.
    #[serde(default)]
    pub vector: VectorSearchSection,
    /// Text-to-vector embedding calls backing vector text queries.
    #[serde(default)]
    pub embedding: EmbeddingSection,
    /// Post-recall rerank provider calls for vector text queries.
    #[serde(default)]
    pub rerank: RerankSection,
}

/// Vector engine search and write counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VectorSearchSection {
    pub total_queries: u64,
    pub total_errors: u64,
    pub total_latency_ms: u64,
    pub avg_latency_ms: f64,
    pub total_upserts: u64,
    pub upsert_errors: u64,
    pub total_deletes: u64,
    pub delete_errors: u64,
    pub disabled_skips: u64,
}

/// Text-to-vector embedding call counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EmbeddingSection {
    pub total_operations: u64,
    pub total_errors: u64,
    pub total_latency_ms: u64,
    pub avg_latency_ms: f64,
    pub prompt_tokens: u64,
    pub total_tokens: u64,
}

/// Rerank provider call counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RerankSection {
    pub total_operations: u64,
    pub total_errors: u64,
    pub total_latency_ms: u64,
    pub avg_latency_ms: f64,
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

// ── Transaction inspection ────────────────────────────────────────────────

/// One active transaction row.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionListItem {
    pub transaction_id: u64,
    pub state: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub read_timestamp: u64,
    #[serde(default)]
    pub write_timestamp: u64,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub last_activity_ms: u64,
    #[serde(default)]
    pub rollback_only: bool,
    #[serde(default)]
    pub staged_bytes: u64,
    #[serde(default)]
    pub undo_bytes: u64,
    #[serde(default)]
    pub blocking_reason: Option<String>,
}

/// Transaction outcome counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionOutcomes {
    #[serde(default)]
    pub begun: u64,
    #[serde(default)]
    pub active: u64,
    #[serde(default)]
    pub committed: u64,
    #[serde(default)]
    pub aborted: u64,
    #[serde(default)]
    pub timeout: u64,
    #[serde(default)]
    pub conflict: u64,
    #[serde(default)]
    pub disconnect: u64,
    #[serde(default)]
    pub recovery_abort: u64,
    #[serde(default)]
    pub cleanup_failure: u64,
}

/// Transaction resource gauges.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionResources {
    #[serde(default)]
    pub active_statements: u64,
    #[serde(default)]
    pub active_snapshots: u64,
    #[serde(default)]
    pub pending_writes: u64,
    #[serde(default)]
    pub committed_frontier_lag: u64,
    #[serde(default)]
    pub staged_wal_bytes: u64,
    #[serde(default)]
    pub undo_bytes: u64,
    #[serde(default)]
    pub checkpoint_drain_time_ms: u64,
}

/// Transaction outcome and resource gauges.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionMetricsResponse {
    #[serde(default)]
    pub outcomes: TransactionOutcomes,
    #[serde(default)]
    pub resources: TransactionResources,
}

/// Transaction commit receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionCommitResponse {
    pub transaction_id: u64,
    pub message: String,
}

/// Transaction rollback receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionRollbackResponse {
    pub transaction_id: u64,
    pub message: String,
}

/// Transaction kill receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionKillResponse {
    pub transaction_id: u64,
    pub status: String,
}

/// Per-transaction outbox retry receipt. The engine replays the global
/// durable queue, so the scope marker stays explicit.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransactionOutboxRetryResponse {
    pub transaction_id: u64,
    pub delivered: u64,
    pub scope: String,
    pub status: String,
}

/// One savepoint row.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SavepointListItem {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

/// Rollback-to-savepoint receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SavepointRollbackResponse {
    pub transaction_id: u64,
    pub savepoint_id: u64,
    pub message: String,
}

/// Savepoint release receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SavepointReleaseResponse {
    pub transaction_id: u64,
    pub savepoint_id: u64,
    pub message: String,
}

// ── Session inspection ────────────────────────────────────────────────────

/// One session row.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionListItem {
    pub session_id: i64,
    pub username: String,
    #[serde(default)]
    pub space_name: Option<String>,
    #[serde(default)]
    pub graph_addr: Option<String>,
    #[serde(default)]
    pub active_queries: u64,
}

/// Session listing payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionListResponse {
    #[serde(default)]
    pub sessions: Vec<SessionListItem>,
}

/// Session detail payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionDetailResponse {
    pub session_id: i64,
    pub username: String,
    #[serde(default)]
    pub space_name: Option<String>,
    #[serde(default)]
    pub graph_addr: Option<String>,
    #[serde(default)]
    pub timezone: Option<i32>,
}

/// Session variable snapshot payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VariableSnapshotResponse {
    pub session_id: i64,
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, crate::query::CellValue>)]
    pub variables: std::collections::HashMap<String, serde_json::Value>,
}

/// Assign one session variable.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetVariableRequest {
    pub name: String,
    #[serde(default)]
    #[schema(value_type = Option<crate::query::CellValue>)]
    pub value: Option<serde_json::Value>,
}

/// Variable assignment receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SetVariableResponse {
    pub session_id: i64,
    pub name: String,
    #[serde(default)]
    #[schema(value_type = Option<crate::query::CellValue>)]
    pub value: Option<serde_json::Value>,
}

/// Variable removal receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeleteVariableResponse {
    pub session_id: i64,
    pub name: String,
    pub removed: bool,
}

// ── Function registry ─────────────────────────────────────────────────────

/// One function row.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FunctionListItem {
    pub name: String,
}

/// Function listing payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FunctionListResponse {
    #[serde(default)]
    pub functions: Vec<FunctionListItem>,
    #[serde(default)]
    pub total: u64,
}

/// Function detail payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FunctionDetailResponse {
    pub name: String,
    #[serde(default, rename = "type")]
    pub function_type: String,
    #[serde(default)]
    pub is_builtin: bool,
    #[serde(default)]
    pub is_custom: bool,
    #[serde(default)]
    pub parameters: Vec<String>,
    #[serde(default)]
    pub return_type: String,
    #[serde(default)]
    pub registered_at: Option<String>,
}

/// Function registration receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FunctionRegisterResponse {
    pub function_id: String,
    pub name: String,
    pub function_type: String,
    pub return_type: String,
    pub status: String,
    pub message: String,
}

/// Function removal receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FunctionUnregisterResponse {
    pub name: String,
    pub message: String,
}

// ── Statistics inspection ─────────────────────────────────────────────────

/// Session statistics detail payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionStatsDetail {
    #[serde(default)]
    pub total_queries: u64,
    #[serde(default)]
    pub total_changes: u64,
    #[serde(default)]
    pub last_insert_vertex_id: Option<u64>,
    #[serde(default)]
    pub last_insert_edge_id: Option<u64>,
    #[serde(default)]
    pub avg_execution_time_ms: f64,
}

/// Session statistics payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionStatsResponse {
    pub session_id: i64,
    pub username: String,
    #[serde(default)]
    pub statistics: SessionStatsDetail,
}

/// Background freeze statistics payload. Disabled deployments report
/// the enabled flag with an explanatory message.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FreezeStatsResponse {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub freeze_count: u64,
    #[serde(default)]
    pub total_frozen_edges: u64,
    #[serde(default)]
    pub last_freeze_duration_ms: u64,
    #[serde(default)]
    pub current_delta_edges: u64,
    #[serde(default)]
    pub message: Option<String>,
}

/// Manual freeze trigger receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FreezeTriggerResponse {
    pub status: String,
    pub message: String,
}

/// Migration engine counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MigrationEngineStats {
    #[serde(default)]
    pub total_migrations: u64,
    #[serde(default)]
    pub successful_migrations: u64,
    #[serde(default)]
    pub failed_migrations: u64,
    #[serde(default)]
    pub total_rows_migrated: u64,
    #[serde(default)]
    pub total_duration_ms: u64,
}

/// Manager-level migration counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MigrationManagerStats {
    #[serde(default)]
    pub total_migrations: u64,
    #[serde(default)]
    pub active_migrations: u64,
    #[serde(default)]
    pub rows_migrated: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub avg_duration_ms: f64,
    #[serde(default)]
    pub errors: u64,
}

/// Migration statistics payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MigrationStatsResponse {
    #[serde(default)]
    pub stats_manager: MigrationManagerStats,
    #[serde(default)]
    pub migration_metrics: MigrationEngineStats,
}

// ── Batch receipts ────────────────────────────────────────────────────────

/// Batch cancel or delete receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BatchActionReceipt {
    pub batch_id: String,
    pub message: String,
}

// ── Fulltext listings ─────────────────────────────────────────────────────

/// Fulltext index listing payload. Entries stay generic because index
/// descriptors span engine-specific shapes.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FulltextIndexListResponse {
    #[serde(default)]
    pub indexes: Vec<serde_json::Value>,
}

/// Inconsistent fulltext index payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct InconsistentFulltextResponse {
    #[serde(default)]
    pub inconsistent: Vec<serde_json::Value>,
    #[serde(default)]
    pub count: u64,
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
