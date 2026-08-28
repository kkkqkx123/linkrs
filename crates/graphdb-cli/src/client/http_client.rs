//! HTTP client for connecting to GraphDB server
//!
//! All request/response bodies use the shared `graphdb-wire` DTOs, so the
//! CLI never maintains a second copy of the HTTP contract.

use std::time::Duration;

use graphdb_wire::batch::{
    AddBatchItemsRequest, AddBatchItemsResponse, BatchItem, BatchStatusResponse, BatchType,
    CreateBatchRequest, CreateBatchResponse, ExecuteBatchResponse,
};
use graphdb_wire::meta::{
    BeginTransactionRequest, ColdSnapshotInfo, DatabaseStatistics, ExportSnapshotRequest,
    LoadSnapshotRequest, LoginRequest, LoginResponse, LogoutRequest, MergeSnapshotsRequest,
    QueryStatistics, ServerConfig, SessionStatistics, TransactionActionRequest,
    TransactionResponse, UpdateConfigRequest,
};
use graphdb_wire::query::{BatchQueryRequest, BatchQueryResponse, QueryRequest, QueryResponse};
use graphdb_wire::schema::{
    CreateEdgeTypeRequest, CreateSpaceRequest, CreateTagRequest, EdgeTypeInfo, PropertyDef,
    SpaceInfo, TagInfo,
};

use crate::client::config::{ClientConfig, SessionInfo};
use crate::client::transaction::TransactionOptions;
use crate::client::types::QueryResult;
use crate::utils::error::{CliError, Result};

/// HTTP client for connecting to remote GraphDB server
pub struct HttpClient {
    inner: reqwest::Client,
    base_url: String,
    config: ClientConfig,
    connected: bool,
    session_info: Option<SessionInfo>,
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new(host: &str, port: u16) -> Result<Self> {
        let config = ClientConfig::new().with_host(host).with_port(port);
        Self::with_config(config)
    }

    /// Create a new HTTP client with custom configuration
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        let base_url = format!("http://{}:{}/v1", config.host, config.port);
        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| CliError::connection(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            inner,
            base_url,
            config,
            connected: false,
            session_info: None,
        })
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get the underlying reqwest client
    pub fn inner(&self) -> &reqwest::Client {
        &self.inner
    }

    /// Check if client is currently connected
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Get connection string
    pub fn connection_string(&self) -> String {
        self.base_url.clone()
    }

    /// Connect to the database
    pub async fn connect(&mut self) -> Result<SessionInfo> {
        let (session_id, username) = self
            .login(&self.config.username, &self.config.password)
            .await?;

        let session_info = SessionInfo {
            session_id,
            username: username.clone(),
            host: self.config.host.clone(),
            port: self.config.port,
        };

        self.session_info = Some(session_info.clone());
        self.connected = true;

        Ok(session_info)
    }

    /// Disconnect from the database
    pub async fn disconnect(&mut self) -> Result<()> {
        if let Some(ref session_info) = self.session_info {
            let url = format!("{}/auth/logout", self.base_url);
            let request = LogoutRequest {
                session_id: session_info.session_id,
            };

            match self.inner.post(&url).json(&request).send().await {
                Ok(response) => {
                    if !response.status().is_success() {
                        let status = response.status();
                        let body = response.text().await.unwrap_or_default();
                        eprintln!("Warning: Logout failed ({}): {}", status, body);
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Failed to contact server during logout: {}", e);
                }
            }
        }

        self.connected = false;
        self.session_info = None;
        Ok(())
    }

    /// Execute a query and return results
    pub async fn execute_query(&self, query: &str, session_id: i64) -> Result<QueryResult> {
        let url = format!("{}/query", self.base_url);
        let request = QueryRequest {
            query: query.to_string(),
            session_id,
            parameters: std::collections::HashMap::new(),
            session_variables: std::collections::HashMap::new(),
            consistency: None,
            consistency_timeout_ms: None,
            minimum_lsn: None,
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Query failed ({}): {}",
                status, body
            )));
        }

        let wire: QueryResponse = response.json().await?;
        Ok(QueryResult::from(wire))
    }

    /// Execute multiple auto-commit DML statements inside a single shared
    /// auto-commit batch window on the server. Returns one
    /// [`QueryResult`] per input statement, in order; failures are reported
    /// per statement (inline `error`) and do not abort the rest of the batch.
    pub async fn execute_query_batch(
        &self,
        statements: &[String],
        session_id: i64,
    ) -> Result<Vec<QueryResult>> {
        if statements.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/query/batch", self.base_url);
        let request = BatchQueryRequest {
            session_id,
            statements: statements.to_vec(),
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Batch query failed ({}): {}",
                status, body
            )));
        }

        let batch_resp: BatchQueryResponse = response.json().await?;
        Ok(batch_resp
            .results
            .into_iter()
            .map(QueryResult::from)
            .collect())
    }

    /// Execute a query without variable substitution
    pub async fn execute_query_raw(&self, query: &str, session_id: i64) -> Result<QueryResult> {
        self.execute_query(query, session_id).await
    }

    /// List all available spaces
    pub async fn list_spaces(&self) -> Result<Vec<SpaceInfo>> {
        let url = format!("{}/schema/spaces", self.base_url);
        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to list spaces ({}): {}",
                status, body
            )));
        }

        let body: serde_json::Value = response.json().await?;
        let spaces = body
            .get("spaces")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(spaces)
    }

    /// Switch to a specific space
    pub async fn switch_space(&self, space: &str) -> Result<()> {
        let url = format!("{}/schema/spaces/{}", self.base_url, space);
        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to use space '{}' ({}): {}",
                space, status, body
            )));
        }

        Ok(())
    }

    /// List all tags in current space
    pub async fn list_tags(&self, space: &str) -> Result<Vec<TagInfo>> {
        let url = format!("{}/schema/spaces/{}/tags", self.base_url, space);
        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to list tags ({}): {}",
                status, body
            )));
        }

        let body: serde_json::Value = response.json().await?;
        let tags = body
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(tags)
    }

    /// List all edge types in current space
    pub async fn list_edge_types(&self, space: &str) -> Result<Vec<EdgeTypeInfo>> {
        let url = format!("{}/schema/spaces/{}/edge-types", self.base_url, space);
        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to list edge types ({}): {}",
                status, body
            )));
        }

        let body: serde_json::Value = response.json().await?;
        let edge_types = body
            .get("edge_types")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(edge_types)
    }

    /// Check server/database health
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url);
        let response = self.inner.get(&url).send().await;
        match response {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    // ── Cold snapshot management ──

    /// List all registered cold snapshots.
    pub async fn list_cold_snapshots(&self) -> Result<Vec<ColdSnapshotInfo>> {
        let url = format!("{}/snapshots/cold", self.base_url);
        let response = self.inner.get(&url).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to list cold snapshots ({}): {}",
                status, body
            )));
        }
        let body: serde_json::Value = response.json().await?;
        let snapshots = body
            .get("snapshots")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        Ok(snapshots)
    }

    /// Register a cold snapshot from a server-side `.lkcs` file.
    pub async fn load_cold_snapshot(&self, path: &str) -> Result<ColdSnapshotInfo> {
        let url = format!("{}/snapshots/cold/load", self.base_url);
        let request = LoadSnapshotRequest {
            path: path.to_string(),
        };
        let response = self.inner.post(&url).json(&request).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to load cold snapshot ({}): {}",
                status, body
            )));
        }
        Ok(response.json().await?)
    }

    /// Drop all cold snapshots of a label from the registry.
    pub async fn remove_cold_snapshot(&self, label: u32) -> Result<()> {
        let url = format!("{}/snapshots/cold/{}", self.base_url, label);
        let response = self.inner.delete(&url).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to remove cold snapshot ({}): {}",
                status, body
            )));
        }
        Ok(())
    }

    /// Re-export the most recent cold snapshot of a label to a path.
    pub async fn export_cold_snapshot(&self, label: u32, path: &str) -> Result<ColdSnapshotInfo> {
        let url = format!("{}/snapshots/cold/export", self.base_url);
        let request = ExportSnapshotRequest {
            label,
            path: path.to_string(),
        };
        let response = self.inner.post(&url).json(&request).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to export cold snapshot ({}): {}",
                status, body
            )));
        }
        Ok(response.json().await?)
    }

    /// Consolidate every registered version of the given labels.
    pub async fn merge_cold_snapshots(&self, labels: &[u32]) -> Result<Vec<ColdSnapshotInfo>> {
        let url = format!("{}/snapshots/cold/merge", self.base_url);
        let request = MergeSnapshotsRequest {
            labels: labels.to_vec(),
        };
        let response = self.inner.post(&url).json(&request).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to merge cold snapshots ({}): {}",
                status, body
            )));
        }
        let body: serde_json::Value = response.json().await?;
        let merged = body
            .get("merged")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        Ok(merged)
    }

    // ── Schema DDL (admin commands) ──

    /// Create a new graph space
    pub async fn create_space(
        &self,
        name: &str,
        vid_type: Option<&str>,
        comment: Option<&str>,
    ) -> Result<()> {
        let url = format!("{}/schema/spaces", self.base_url);
        let request = CreateSpaceRequest {
            name: name.to_string(),
            vid_type: vid_type.map(|s| s.to_string()),
            comment: comment.map(|s| s.to_string()),
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to create space ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    /// Drop a graph space
    pub async fn drop_space(&self, name: &str) -> Result<()> {
        let url = format!("{}/schema/spaces/{}", self.base_url, name);

        let response = self.inner.delete(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to drop space ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    /// Create a tag in a space
    pub async fn create_tag(
        &self,
        space: &str,
        name: &str,
        properties: Vec<PropertyDef>,
    ) -> Result<()> {
        let url = format!("{}/schema/spaces/{}/tags", self.base_url, space);

        let request = CreateTagRequest {
            name: name.to_string(),
            properties,
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to create tag ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    /// Create an edge type in a space
    pub async fn create_edge_type(
        &self,
        space: &str,
        name: &str,
        properties: Vec<PropertyDef>,
    ) -> Result<()> {
        let url = format!("{}/schema/spaces/{}/edge-types", self.base_url, space);

        let request = CreateEdgeTypeRequest {
            name: name.to_string(),
            properties,
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to create edge type ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    // ── Transactions ──

    /// Begin a new transaction
    pub async fn begin_transaction(
        &self,
        options: TransactionOptions,
    ) -> Result<TransactionResponse> {
        let url = format!("{}/transactions", self.base_url);

        let request = BeginTransactionRequest {
            read_only: options.read_only,
            timeout_seconds: options.timeout_seconds,
            query_timeout_seconds: None,
            statement_timeout_seconds: None,
            idle_timeout_seconds: None,
            isolation_level: None,
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::transaction(format!(
                "Failed to begin transaction ({}): {}",
                status, body
            )));
        }

        Ok(response.json().await?)
    }

    /// Commit a transaction
    pub async fn commit_transaction(&self, txn_id: u64) -> Result<()> {
        let url = format!("{}/transactions/{}/commit", self.base_url, txn_id);

        let session_id = self
            .session_info
            .as_ref()
            .map(|s| s.session_id)
            .ok_or_else(|| CliError::session("Not connected".to_string()))?;

        let request = TransactionActionRequest { session_id };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::transaction(format!(
                "Failed to commit transaction ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    /// Rollback a transaction
    pub async fn rollback_transaction(&self, txn_id: u64) -> Result<()> {
        let url = format!("{}/transactions/{}/rollback", self.base_url, txn_id);

        let session_id = self
            .session_info
            .as_ref()
            .map(|s| s.session_id)
            .ok_or_else(|| CliError::session("Not connected".to_string()))?;

        let request = TransactionActionRequest { session_id };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::transaction(format!(
                "Failed to rollback transaction ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    // ── Batch import (admin commands) ──

    /// Create a batch task
    pub async fn create_batch(
        &self,
        space_id: u64,
        batch_type: BatchType,
        batch_size: usize,
    ) -> Result<String> {
        let url = format!("{}/batch", self.base_url);

        let request = CreateBatchRequest {
            space_id,
            batch_type,
            batch_size,
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to create batch ({}): {}",
                status, body
            )));
        }

        let batch_resp: CreateBatchResponse = response.json().await?;
        Ok(batch_resp.batch_id)
    }

    /// Add items to a batch
    pub async fn add_batch_items(&self, batch_id: &str, items: Vec<BatchItem>) -> Result<usize> {
        let url = format!("{}/batch/{}/items", self.base_url, batch_id);

        let request = AddBatchItemsRequest { items };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to add batch items ({}): {}",
                status, body
            )));
        }

        let add_resp: AddBatchItemsResponse = response.json().await?;
        Ok(add_resp.accepted)
    }

    /// Execute a batch task
    pub async fn execute_batch(&self, batch_id: &str) -> Result<ExecuteBatchResponse> {
        let url = format!("{}/batch/{}/execute", self.base_url, batch_id);

        let response = self.inner.post(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to execute batch ({}): {}",
                status, body
            )));
        }

        Ok(response.json().await?)
    }

    /// Get batch status
    pub async fn get_batch_status(&self, batch_id: &str) -> Result<BatchStatusResponse> {
        let url = format!("{}/batch/{}", self.base_url, batch_id);

        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to get batch status ({}): {}",
                status, body
            )));
        }

        Ok(response.json().await?)
    }

    /// Cancel a batch task
    pub async fn cancel_batch(&self, batch_id: &str) -> Result<()> {
        let url = format!("{}/batch/{}/cancel", self.base_url, batch_id);

        let response = self.inner.post(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to cancel batch ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    // ── Statistics ──

    /// Get session statistics
    pub async fn get_session_statistics(&self, session_id: i64) -> Result<SessionStatistics> {
        let url = format!("{}/statistics/sessions/{}", self.base_url, session_id);

        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to get session statistics ({}): {}",
                status, body
            )));
        }

        Ok(response.json().await?)
    }

    /// Get query statistics
    pub async fn get_query_statistics(&self) -> Result<QueryStatistics> {
        let url = format!("{}/statistics/queries", self.base_url);

        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to get query statistics ({}): {}",
                status, body
            )));
        }

        Ok(response.json().await?)
    }

    /// Get database statistics
    pub async fn get_database_statistics(&self) -> Result<DatabaseStatistics> {
        let url = format!("{}/statistics/database", self.base_url);

        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to get database statistics ({}): {}",
                status, body
            )));
        }

        Ok(response.json().await?)
    }

    // ── Server configuration ──

    /// Get server configuration
    pub async fn get_config(&self) -> Result<ServerConfig> {
        let url = format!("{}/config", self.base_url);

        let response = self.inner.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to get config ({}): {}",
                status, body
            )));
        }

        Ok(response.json().await?)
    }

    /// Update server configuration
    pub async fn update_config(
        &self,
        section: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        let url = format!("{}/config", self.base_url);

        let request = UpdateConfigRequest {
            section: section.to_string(),
            key: key.to_string(),
            value,
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to update config ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    // ── Auth ──

    /// Login and authenticate (low-level API)
    async fn login(&self, username: &str, password: &str) -> Result<(i64, String)> {
        let url = format!("{}/auth/login", self.base_url);
        let request = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::auth(format!(
                "Login failed ({}): {}",
                status, body
            )));
        }

        let login_resp: LoginResponse = response.json().await?;
        Ok((login_resp.session_id, login_resp.username))
    }
}
