//! Meta contract DTOs: auth, session, transaction, config, statistics and
//! cold-snapshot endpoints.
//!
//! The CLI-side mirrors (`cli/client/{config_types,stats,snapshot,
//! transaction}.rs` and parts of `request_types.rs`) are consolidated here.

use serde::{Deserialize, Serialize};

// ── Auth ──────────────────────────────────────────────────────────────────

/// Login request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub session_id: i64,
    pub username: String,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

/// Logout request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogoutRequest {
    pub session_id: i64,
}

// ── Session ───────────────────────────────────────────────────────────────

/// Create session request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub username: String,
    pub client_ip: String,
}

/// Session response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    pub session_id: i64,
    pub username: String,
    #[serde(default)]
    pub created_at: u64,
}

// ── Transaction ───────────────────────────────────────────────────────────

/// Begin transaction request
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionResponse {
    pub transaction_id: u64,
    pub status: String,
}

/// Transaction action request (commit/rollback body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionActionRequest {
    pub session_id: i64,
}

// ── Config ────────────────────────────────────────────────────────────────

/// Update config request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfigRequest {
    pub section: String,
    pub key: String,
    pub value: serde_json::Value,
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub version: String,
    #[serde(default)]
    pub sections: Vec<ConfigSection>,
}

/// Configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSection {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub items: Vec<ConfigItem>,
}

/// Configuration item
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatistics {
    pub total_queries: u64,
    pub total_changes: u64,
    pub avg_execution_time_ms: f64,
}

/// Query type statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryStatistics {
    pub total_queries: u64,
    #[serde(default)]
    pub slow_queries: Vec<SlowQueryInfo>,
    pub query_types: QueryTypeStatistics,
}

/// Information about a slow query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowQueryInfo {
    pub trace_id: String,
    pub session_id: i64,
    pub query: String,
    pub duration_ms: f64,
    pub status: String,
}

/// Database statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseStatistics {
    pub space_count: i64,
    pub total_vertices: i64,
    pub total_edges: i64,
    pub total_queries: u64,
    pub active_queries: u64,
    pub queries_per_second: f64,
    pub avg_latency_ms: f64,
}

// ── Cold snapshots ────────────────────────────────────────────────────────

/// Metadata describing one registered cold snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColdSnapshotInfo {
    pub label: u32,
    #[serde(default)]
    pub label_name: String,
    #[serde(default)]
    pub snapshot_ts: u64,
    #[serde(default)]
    pub edge_count: u64,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub checksum: u32,
}

/// Load cold snapshot request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadSnapshotRequest {
    pub path: String,
}

/// Export cold snapshot request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSnapshotRequest {
    pub label: u32,
    pub path: String,
}

/// Merge cold snapshots request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeSnapshotsRequest {
    pub labels: Vec<u32>,
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
    fn cold_snapshot_info_roundtrip() {
        let info = ColdSnapshotInfo {
            label: 1,
            label_name: "person".to_string(),
            snapshot_ts: 100,
            edge_count: 5,
            file_path: "/tmp/x.lkcs".to_string(),
            file_size: 1024,
            checksum: 0xDEAD,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: ColdSnapshotInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.label, 1);
        assert_eq!(back.edge_count, 5);
        assert_eq!(back.checksum, 0xDEAD);
    }

    #[test]
    fn cold_snapshot_info_missing_fields_default() {
        let back: ColdSnapshotInfo = serde_json::from_str(r#"{"label": 3}"#).unwrap();
        assert_eq!(back.label, 3);
        assert_eq!(back.edge_count, 0);
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
    fn snapshot_requests_roundtrip() {
        let load = LoadSnapshotRequest {
            path: "/tmp/a.lkcs".to_string(),
        };
        let back: LoadSnapshotRequest =
            serde_json::from_str(&serde_json::to_string(&load).unwrap()).unwrap();
        assert_eq!(back.path, "/tmp/a.lkcs");

        let merge = MergeSnapshotsRequest { labels: vec![1, 2] };
        let back: MergeSnapshotsRequest =
            serde_json::from_str(&serde_json::to_string(&merge).unwrap()).unwrap();
        assert_eq!(back.labels, vec![1, 2]);
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
