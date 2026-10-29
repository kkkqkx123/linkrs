//! HTTP client for connecting to Linkrs server
//!
//! All request/response bodies use the shared `linkrs-wire` DTOs, so the
//! CLI never maintains a second copy of the HTTP contract.

use std::time::Duration;

use linkrs_wire::batch::{
    AddBatchItemsRequest, AddBatchItemsResponse, BatchItem, BatchStatusResponse, BatchType,
    CreateBatchRequest, CreateBatchResponse, ExecuteBatchResponse,
};
use linkrs_wire::meta::{
    BeginTransactionRequest, DatabaseStatistics, LoginRequest, LoginResponse, LogoutRequest,
    QueryStatistics, SessionStatistics, TransactionActionRequest, TransactionResponse,
};
use linkrs_wire::query::{BatchQueryRequest, BatchQueryResponse, QueryRequest, QueryResponse};
use linkrs_wire::schema::{
    CreateEdgeTypeRequest, CreateSpaceRequest, CreateTagRequest, EdgeTypeInfo, PropertyDef,
    SpaceInfo, TagInfo,
};

use crate::config::{ClientConfig, SessionInfo};
use crate::transaction::TransactionOptions;
use crate::types::QueryResult;
use crate::utils::error::{ClientError, Result};

/// HTTP client for connecting to remote Linkrs server
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
            .map_err(|e| ClientError::connection(format!("Failed to create HTTP client: {}", e)))?;

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

    /// Active session identifier for authenticated requests.
    pub fn session_id(&self) -> Option<i64> {
        self.session_info.as_ref().map(|s| s.session_id)
    }

    /// Attach the session header required by protected routes.
    pub(crate) fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.session_id() {
            Some(id) => builder.header("X-Session-ID", id),
            None => builder,
        }
    }

    /// Reject non-2xx responses with a shared [`ClientError::Http`] carrying the
    /// caller-supplied operation context plus the server body.
    pub async fn check_response(
        response: reqwest::Response,
        context: impl Into<String>,
    ) -> Result<reqwest::Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(ClientError::http(
            status.as_u16(),
            format!("{} ({}): {}", context.into(), status, body),
        ))
    }

    /// Drop the stored password so it does not linger after disconnect.
    pub fn clear_credentials(&mut self) {
        self.config.password.clear();
    }

    /// Connect to the database
    pub async fn connect(&mut self) -> Result<SessionInfo> {
        let login = self
            .login(&self.config.username, &self.config.password)
            .await?;

        let session_info = SessionInfo {
            session_id: login.session_id,
            username: login.username.clone(),
            host: self.config.host.clone(),
            port: self.config.port,
            roles: login.roles,
            display_role: login.display_role,
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

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Query failed").await?;

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
            parameters: std::collections::HashMap::new(),
            session_variables: std::collections::HashMap::new(),
        };

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Batch query failed").await?;

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
        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to list spaces").await?;

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
        let response = self.authed(self.inner.get(&url)).send().await?;

        Self::check_response(response, format!("Failed to use space '{}'", space)).await?;

        Ok(())
    }

    /// List all tags in current space
    pub async fn list_tags(&self, space: &str) -> Result<Vec<TagInfo>> {
        let url = format!("{}/schema/spaces/{}/tags", self.base_url, space);
        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to list tags").await?;

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
        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to list edge types").await?;

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

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        Self::check_response(response, "Failed to create space").await?;

        Ok(())
    }

    /// Drop a graph space
    pub async fn drop_space(&self, name: &str) -> Result<()> {
        let url = format!("{}/schema/spaces/{}", self.base_url, name);

        let response = self.authed(self.inner.delete(&url)).send().await?;

        Self::check_response(response, "Failed to drop space").await?;

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

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        Self::check_response(response, "Failed to create tag").await?;

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

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        Self::check_response(response, "Failed to create edge type").await?;

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

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Failed to begin transaction").await?;

        Ok(response.json().await?)
    }

    /// Commit a transaction
    pub async fn commit_transaction(&self, txn_id: u64) -> Result<()> {
        let url = format!("{}/transactions/{}/commit", self.base_url, txn_id);

        let session_id = self
            .session_info
            .as_ref()
            .map(|s| s.session_id)
            .ok_or_else(|| ClientError::session("Not connected".to_string()))?;

        let request = TransactionActionRequest { session_id };

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        Self::check_response(response, "Failed to commit transaction").await?;

        Ok(())
    }

    /// List server-side transactions (`GET /v1/transactions`).
    ///
    /// Unlike the local `\txstatus` view, this reflects the server truth
    /// across all sessions and owners.
    pub async fn list_transactions(&self) -> Result<Vec<serde_json::Value>> {
        let url = format!("{}/transactions", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to list transactions").await?;
        Ok(response.json().await?)
    }

    /// Fetch transaction outcome and resource gauges
    /// (`GET /v1/transactions/metrics`).
    pub async fn transaction_metrics(&self) -> Result<serde_json::Value> {
        let url = format!("{}/transactions/metrics", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get transaction metrics").await?;
        Ok(response.json().await?)
    }

    /// Kill a server-side transaction (`POST /v1/transactions/{id}/kill`).
    pub async fn kill_transaction(&self, txn_id: u64) -> Result<serde_json::Value> {
        let url = format!("{}/transactions/{}/kill", self.base_url, txn_id);
        let response = self.authed(self.inner.post(&url)).send().await?;
        let response =
            Self::check_response(response, format!("Failed to kill transaction {}", txn_id))
                .await?;
        Ok(response.json().await?)
    }

    /// Retry outbox projection scoped to one transaction
    /// (`POST /v1/transactions/{id}/outbox/retry`).
    ///
    /// Note: the current engine performs a global retry pass; the id is
    /// validated server-side so unknown ids fail instead of silently
    /// reporting a global count.
    pub async fn retry_transaction_outbox(&self, txn_id: u64) -> Result<serde_json::Value> {
        let url = format!("{}/transactions/{}/outbox/retry", self.base_url, txn_id);
        let response = self.authed(self.inner.post(&url)).send().await?;
        let response = Self::check_response(
            response,
            format!("Failed to retry outbox for transaction {}", txn_id),
        )
        .await?;
        Ok(response.json().await?)
    }

    /// Rollback a transaction
    pub async fn rollback_transaction(&self, txn_id: u64) -> Result<()> {
        let url = format!("{}/transactions/{}/rollback", self.base_url, txn_id);

        let session_id = self
            .session_info
            .as_ref()
            .map(|s| s.session_id)
            .ok_or_else(|| ClientError::session("Not connected".to_string()))?;

        let request = TransactionActionRequest { session_id };

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        Self::check_response(response, "Failed to rollback transaction").await?;

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

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Failed to create batch").await?;

        let batch_resp: CreateBatchResponse = response.json().await?;
        Ok(batch_resp.batch_id)
    }

    /// Add items to a batch
    pub async fn add_batch_items(&self, batch_id: &str, items: Vec<BatchItem>) -> Result<usize> {
        let url = format!("{}/batch/{}/items", self.base_url, batch_id);

        let request = AddBatchItemsRequest { items };

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Failed to add batch items").await?;

        let add_resp: AddBatchItemsResponse = response.json().await?;
        Ok(add_resp.accepted)
    }

    /// Execute a batch task
    pub async fn execute_batch(&self, batch_id: &str) -> Result<ExecuteBatchResponse> {
        let url = format!("{}/batch/{}/execute", self.base_url, batch_id);

        let response = self.authed(self.inner.post(&url)).send().await?;

        let response = Self::check_response(response, "Failed to execute batch").await?;

        Ok(response.json().await?)
    }

    /// Get batch status
    pub async fn get_batch_status(&self, batch_id: &str) -> Result<BatchStatusResponse> {
        let url = format!("{}/batch/{}", self.base_url, batch_id);

        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to get batch status").await?;

        Ok(response.json().await?)
    }

    /// Cancel a batch task
    pub async fn cancel_batch(&self, batch_id: &str) -> Result<()> {
        let url = format!("{}/batch/{}/cancel", self.base_url, batch_id);

        let response = self.authed(self.inner.post(&url)).send().await?;

        Self::check_response(response, "Failed to cancel batch").await?;

        Ok(())
    }

    /// Delete a batch task
    pub async fn delete_batch(&self, batch_id: &str) -> Result<()> {
        let url = format!("{}/batch/{}", self.base_url, batch_id);

        let response = self.authed(self.inner.delete(&url)).send().await?;

        Self::check_response(response, "Failed to delete batch").await?;

        Ok(())
    }

    // ── Statistics ──

    /// Get session statistics
    pub async fn get_session_statistics(&self, session_id: i64) -> Result<SessionStatistics> {
        let url = format!("{}/statistics/sessions/{}", self.base_url, session_id);

        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to get session statistics").await?;

        Ok(response.json().await?)
    }

    /// Get query statistics
    pub async fn get_query_statistics(&self) -> Result<QueryStatistics> {
        let url = format!("{}/statistics/queries", self.base_url);

        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to get query statistics").await?;

        Ok(response.json().await?)
    }

    /// Get database statistics
    pub async fn get_database_statistics(&self) -> Result<DatabaseStatistics> {
        let url = format!("{}/statistics/database", self.base_url);

        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to get database statistics").await?;

        Ok(response.json().await?)
    }

    // ── Server configuration ──

    /// Get server configuration (flat section map as served by `GET /v1/config`).
    pub async fn get_config(&self) -> Result<serde_json::Value> {
        let url = format!("{}/config", self.base_url);

        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to get config").await?;

        Ok(response.json().await?)
    }

    /// Get a single configuration item via `GET /v1/config/{section}/{key}`.
    pub async fn get_config_key(&self, section: &str, key: &str) -> Result<serde_json::Value> {
        let url = format!("{}/config/{}/{}", self.base_url, section, key);

        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to get config item").await?;

        Ok(response.json().await?)
    }

    /// Update a single configuration item via `PUT /v1/config/{section}/{key}`.
    pub async fn update_config(
        &self,
        section: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/config/{}/{}", self.base_url, section, key);

        let response = self
            .authed(self.inner.put(&url))
            .json(&serde_json::json!({ "value": value }))
            .send()
            .await?;

        let response = Self::check_response(response, "Failed to update config").await?;

        Ok(response.json().await?)
    }

    /// Reset a single configuration item via `DELETE /v1/config/{section}/{key}`.
    pub async fn reset_config(&self, section: &str, key: &str) -> Result<serde_json::Value> {
        let url = format!("{}/config/{}/{}", self.base_url, section, key);

        let response = self.authed(self.inner.delete(&url)).send().await?;

        let response = Self::check_response(response, "Failed to reset config").await?;

        Ok(response.json().await?)
    }

    /// Get system resource statistics via `GET /v1/statistics/system`.
    pub async fn get_system_statistics(&self) -> Result<serde_json::Value> {
        let url = format!("{}/statistics/system", self.base_url);

        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to get system statistics").await?;

        Ok(response.json().await?)
    }

    /// Get cross-subsystem overview via `GET /v1/statistics/overview`.
    pub async fn get_overview(&self) -> Result<serde_json::Value> {
        let url = format!("{}/statistics/overview", self.base_url);

        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to get overview").await?;

        Ok(response.json().await?)
    }

    /// Execute a query in streaming mode (`POST /v1/query/stream`, SSE).
    /// Rows are collected as they arrive and returned as one [`QueryResult`],
    /// so large results avoid the materialized row ceiling of `/v1/query`.
    pub async fn execute_query_stream(&self, query: &str, session_id: i64) -> Result<QueryResult> {
        use linkrs_wire::query::StreamQueryRequest;
        let url = format!("{}/query/stream", self.base_url);
        let request = StreamQueryRequest {
            query: query.to_string(),
            session_id,
            event_buffer_capacity: 100,
            statements: Vec::new(),
            parameters: std::collections::HashMap::new(),
            session_variables: std::collections::HashMap::new(),
            fail_fast: true,
        };

        let response = self
            .authed(self.inner.post(&url))
            .header("Accept", "text/event-stream")
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Stream query failed").await?;

        let text = response.text().await?;
        let mut parser = crate::sse::SseParser::new();
        for line in text.lines() {
            parser.feed_line(line);
        }
        parser.finish()
    }

    /// Open a forward-only cursor over a query via `POST /v1/query/cursor/open`.
    pub async fn open_cursor(&self, query: &str, session_id: i64) -> Result<(u64, Vec<String>)> {
        use linkrs_wire::query::{OpenCursorRequest, OpenCursorResponse};
        let url = format!("{}/query/cursor/open", self.base_url);
        let request = OpenCursorRequest {
            session_id,
            query: query.to_string(),
        };

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Open cursor failed").await?;

        let body: OpenCursorResponse = response.json().await?;
        Ok((body.cursor_id, body.columns))
    }

    /// Fetch a cursor page with the exhaustion flag.
    pub async fn fetch_cursor_page(
        &self,
        cursor_id: u64,
        page_size: usize,
        session_id: i64,
    ) -> Result<(QueryResult, bool)> {
        use linkrs_wire::query::{FetchCursorRequest, FetchCursorResponse};
        let url = format!("{}/query/cursor/fetch", self.base_url);
        let request = FetchCursorRequest {
            session_id,
            cursor_id,
            page_size,
        };

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Fetch cursor failed").await?;

        let body: FetchCursorResponse = response.json().await?;
        let has_more = body.has_more;
        let row_count = body.rows.len();
        Ok((
            QueryResult {
                columns: body.columns,
                rows: body.rows,
                row_count,
                execution_time_ms: 0,
                rows_scanned: 0,
                trace_id: body.trace_id,
                error: None,
            },
            has_more,
        ))
    }

    /// Release a cursor via `POST /v1/query/cursor/close`.
    pub async fn close_cursor(&self, cursor_id: u64, session_id: i64) -> Result<bool> {
        use linkrs_wire::query::{CloseCursorRequest, CloseCursorResponse};
        let url = format!("{}/query/cursor/close", self.base_url);
        let request = CloseCursorRequest {
            session_id,
            cursor_id,
        };

        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;

        let response = Self::check_response(response, "Close cursor failed").await?;

        let body: CloseCursorResponse = response.json().await?;
        Ok(body.closed)
    }

    /// List all functions registered on the server.
    ///
    /// The server exposes the live function registry; names include both
    /// builtins and loaded custom UDFs.
    pub async fn list_functions(&self) -> Result<Vec<String>> {
        let url = format!("{}/functions", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;

        let response = Self::check_response(response, "Failed to list functions").await?;

        let body: serde_json::Value = response.json().await?;
        let names = body
            .get("functions")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(names)
    }

    // ── User management ──

    /// List users via `GET /v1/users` (admin only).
    pub async fn list_users(&self) -> Result<linkrs_wire::meta::UserListResponse> {
        let url = format!("{}/users", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to list users").await?;
        Ok(response.json().await?)
    }

    /// Create a user via `POST /v1/users` (admin only).
    pub async fn create_user(&self, username: &str, password: &str) -> Result<()> {
        let url = format!("{}/users", self.base_url);
        let request = linkrs_wire::meta::CreateUserRequest {
            username: username.to_string(),
            password: password.to_string(),
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        Self::check_response(response, "Failed to create user").await?;
        Ok(())
    }

    /// Drop a user via `DELETE /v1/users/{name}` (admin only).
    pub async fn drop_user(&self, name: &str) -> Result<()> {
        let url = format!("{}/users/{}", self.base_url, name);
        let response = self.authed(self.inner.delete(&url)).send().await?;
        Self::check_response(response, format!("Failed to drop user '{}'", name)).await?;
        Ok(())
    }

    /// Reset a user's password via `POST /v1/users/{name}/password` (admin only).
    pub async fn reset_user_password(&self, name: &str, new_password: &str) -> Result<()> {
        let url = format!("{}/users/{}/password", self.base_url, name);
        let request = linkrs_wire::meta::ResetPasswordRequest {
            password: new_password.to_string(),
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        Self::check_response(response, format!("Failed to reset password for '{}'", name)).await?;
        Ok(())
    }

    /// Enable a user via `POST /v1/users/{name}/enable` (admin only).
    pub async fn enable_user(&self, name: &str) -> Result<()> {
        let url = format!("{}/users/{}/enable", self.base_url, name);
        let response = self.authed(self.inner.post(&url)).send().await?;
        Self::check_response(response, format!("Failed to enable user '{}'", name)).await?;
        Ok(())
    }

    /// Disable a user via `POST /v1/users/{name}/disable` (admin only).
    pub async fn disable_user(&self, name: &str) -> Result<()> {
        let url = format!("{}/users/{}/disable", self.base_url, name);
        let response = self.authed(self.inner.post(&url)).send().await?;
        Self::check_response(response, format!("Failed to disable user '{}'", name)).await?;
        Ok(())
    }

    /// Grant a role on a space via `POST /v1/users/{name}/grant` (admin only).
    pub async fn grant_user_role(&self, name: &str, role: &str, space: &str) -> Result<()> {
        let url = format!("{}/users/{}/grant", self.base_url, name);
        let request = linkrs_wire::meta::GrantRoleRequest {
            role: role.to_string(),
            space: space.to_string(),
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        Self::check_response(response, format!("Failed to grant role to '{}'", name)).await?;
        Ok(())
    }

    /// Revoke a role on a space via `POST /v1/users/{name}/revoke` (admin only).
    pub async fn revoke_user_role(&self, name: &str, space: &str) -> Result<()> {
        let url = format!("{}/users/{}/revoke", self.base_url, name);
        let request = linkrs_wire::meta::RevokeRoleRequest {
            space: space.to_string(),
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        Self::check_response(response, format!("Failed to revoke role from '{}'", name)).await?;
        Ok(())
    }

    // ── Server session management ──

    /// List active server sessions via `GET /v1/sessions` (admin only).
    pub async fn list_server_sessions(&self) -> Result<Vec<serde_json::Value>> {
        let url = format!("{}/sessions", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to list sessions").await?;
        let body: serde_json::Value = response.json().await?;
        Ok(body
            .get("sessions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Get one server session via `GET /v1/sessions/{id}` (admin only).
    pub async fn get_server_session(&self, session_id: i64) -> Result<serde_json::Value> {
        let url = format!("{}/sessions/{}", self.base_url, session_id);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get session").await?;
        Ok(response.json().await?)
    }

    /// Kill a server session via `DELETE /v1/sessions/{id}` (admin only).
    pub async fn kill_server_session(&self, session_id: i64) -> Result<()> {
        let url = format!("{}/sessions/{}", self.base_url, session_id);
        let response = self.authed(self.inner.delete(&url)).send().await?;
        Self::check_response(response, format!("Failed to kill session {}", session_id)).await?;
        Ok(())
    }

    /// Refresh current identity via `GET /v1/auth/me`.
    pub async fn auth_me(&self) -> Result<linkrs_wire::meta::AuthMeResponse> {
        let url = format!("{}/auth/me", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to fetch current identity").await?;
        Ok(response.json().await?)
    }

    // ── Query tooling ──

    /// Validate a GQL statement without executing it via
    /// `POST /v1/query/validate`.
    pub async fn validate_query(&self, query: &str, session_id: i64) -> Result<serde_json::Value> {
        use linkrs_wire::query::ValidateRequest;
        let url = format!("{}/query/validate", self.base_url);
        let request = ValidateRequest {
            query: query.to_string(),
            session_id,
            need_estimate: true,
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        let response = Self::check_response(response, "Validate failed").await?;
        Ok(response.json().await?)
    }

    /// Plan a statement without executing it via `POST /v1/query/explain`.
    pub async fn explain_query(&self, query: &str, session_id: i64) -> Result<QueryResult> {
        use linkrs_wire::query::ExplainRequest;
        let url = format!("{}/query/explain", self.base_url);
        let request = ExplainRequest {
            query: query.to_string(),
            session_id,
            parameters: std::collections::HashMap::new(),
            session_variables: std::collections::HashMap::new(),
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        let response = Self::check_response(response, "Explain failed").await?;
        let wire: linkrs_wire::query::QueryResponse = response.json().await?;
        Ok(QueryResult::from(wire))
    }

    /// Fetch one query portrait via `GET /v1/statistics/queries/{trace_id}`.
    pub async fn get_query_profile_detail(
        &self,
        trace_id: &str,
    ) -> Result<linkrs_wire::meta::QueryProfileDetailResponse> {
        let url = format!("{}/statistics/queries/{}", self.base_url, trace_id);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to fetch query profile").await?;
        Ok(response.json().await?)
    }

    // ── Extended statistics ──

    /// Get per-index search statistics via `GET /v1/statistics/search`.
    pub async fn get_search_statistics(&self) -> Result<serde_json::Value> {
        let url = format!("{}/statistics/search", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get search statistics").await?;
        Ok(response.json().await?)
    }

    /// Get freeze statistics via `GET /v1/statistics/freeze`.
    pub async fn get_freeze_statistics(&self) -> Result<serde_json::Value> {
        let url = format!("{}/statistics/freeze", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get freeze statistics").await?;
        Ok(response.json().await?)
    }

    /// Trigger a manual freeze via `POST /v1/statistics/freeze`.
    pub async fn trigger_freeze(&self) -> Result<serde_json::Value> {
        let url = format!("{}/statistics/freeze", self.base_url);
        let response = self.authed(self.inner.post(&url)).send().await?;
        let response = Self::check_response(response, "Failed to trigger freeze").await?;
        Ok(response.json().await?)
    }

    /// Get migration statistics via `GET /v1/statistics/migration`.
    pub async fn get_migration_statistics(&self) -> Result<serde_json::Value> {
        let url = format!("{}/statistics/migration", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get migration statistics").await?;
        Ok(response.json().await?)
    }

    // ── Function management ──

    /// Get function details via `GET /v1/functions/{name}`.
    pub async fn get_function_info(&self, name: &str) -> Result<serde_json::Value> {
        let url = format!("{}/functions/{}", self.base_url, name);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get function info").await?;
        Ok(response.json().await?)
    }

    /// Unregister a custom function via `DELETE /v1/functions/{name}`.
    pub async fn unregister_function(&self, name: &str) -> Result<()> {
        let url = format!("{}/functions/{}", self.base_url, name);
        let response = self.authed(self.inner.delete(&url)).send().await?;
        Self::check_response(
            response,
            format!("Failed to unregister function '{}'", name),
        )
        .await?;
        Ok(())
    }

    /// Register a custom function via `POST /v1/functions`.
    pub async fn register_function(
        &self,
        name: &str,
        implementation: &str,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/functions", self.base_url);
        let body = serde_json::json!({
            "name": name,
            "implementation": implementation,
        });
        let response = self
            .authed(self.inner.post(&url))
            .json(&body)
            .send()
            .await?;
        let response = Self::check_response(response, "Failed to register function").await?;
        Ok(response.json().await?)
    }

    // ── Schema versioning ──

    /// Get schema version history via `GET /v1/schema/versions/{space}/{label}`.
    pub async fn get_schema_version_history(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
    ) -> Result<serde_json::Value> {
        let mut url = format!("{}/schema/versions/{}/{}", self.base_url, space, label);
        if is_edge {
            url.push_str("?is_edge=true");
        }
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response =
            Self::check_response(response, "Failed to get schema version history").await?;
        Ok(response.json().await?)
    }

    /// Get schema changes between versions via
    /// `GET /v1/schema/changes/{space}/{label}/{from}/{to}`.
    pub async fn get_schema_changes(
        &self,
        space: &str,
        label: &str,
        from_version: u64,
        to_version: u64,
        is_edge: bool,
    ) -> Result<serde_json::Value> {
        let mut url = format!(
            "{}/schema/changes/{}/{}/{}/{}",
            self.base_url, space, label, from_version, to_version
        );
        if is_edge {
            url.push_str("?is_edge=true");
        }
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get schema changes").await?;
        Ok(response.json().await?)
    }

    /// Detect breaking schema changes via
    /// `GET /v1/schema/breaking-changes/{space}/{label}/{from}/{to}`.
    pub async fn detect_schema_breaking_changes(
        &self,
        space: &str,
        label: &str,
        from_version: u64,
        to_version: u64,
        is_edge: bool,
    ) -> Result<serde_json::Value> {
        let mut url = format!(
            "{}/schema/breaking-changes/{}/{}/{}/{}",
            self.base_url, space, label, from_version, to_version
        );
        if is_edge {
            url.push_str("?is_edge=true");
        }
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to detect breaking changes").await?;
        Ok(response.json().await?)
    }

    // ── Fulltext search management ──

    /// Search a fulltext index via `POST /v1/fulltext/search`.
    pub async fn fulltext_search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/fulltext/search", self.base_url);
        let body = serde_json::json!({
            "space_id": space_id,
            "tag_name": tag_name,
            "field_name": field_name,
            "query": query,
            "limit": limit,
        });
        let response = self
            .authed(self.inner.post(&url))
            .json(&body)
            .send()
            .await?;
        let response = Self::check_response(response, "Fulltext search failed").await?;
        Ok(response.json().await?)
    }

    /// List fulltext indexes via `GET /v1/fulltext/indexes`.
    pub async fn list_fulltext_indexes(&self) -> Result<serde_json::Value> {
        let url = format!("{}/fulltext/indexes", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to list fulltext indexes").await?;
        Ok(response.json().await?)
    }

    /// List inconsistent fulltext indexes via `GET /v1/fulltext/indexes/inconsistent`.
    pub async fn list_inconsistent_fulltext_indexes(&self) -> Result<serde_json::Value> {
        let url = format!("{}/fulltext/indexes/inconsistent", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response =
            Self::check_response(response, "Failed to list inconsistent fulltext indexes").await?;
        Ok(response.json().await?)
    }

    /// Start an async fulltext index rebuild via
    /// `POST /v1/fulltext/indexes/rebuild`.
    pub async fn rebuild_fulltext_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<serde_json::Value> {
        use linkrs_wire::fulltext::RebuildFulltextIndexRequest;
        let url = format!("{}/fulltext/indexes/rebuild", self.base_url);
        let request = RebuildFulltextIndexRequest {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: field_name.to_string(),
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        let response = Self::check_response(response, "Failed to start fulltext rebuild").await?;
        Ok(response.json().await?)
    }

    /// Poll a fulltext rebuild via `GET /v1/fulltext/rebuilds/{id}`.
    pub async fn fulltext_rebuild_status(&self, rebuild_id: &str) -> Result<serde_json::Value> {
        let url = format!("{}/fulltext/rebuilds/{}", self.base_url, rebuild_id);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get rebuild status").await?;
        Ok(response.json().await?)
    }

    /// Clear all documents of a fulltext index via
    /// `POST /v1/fulltext/indexes/clear` (destructive).
    pub async fn clear_fulltext_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> Result<serde_json::Value> {
        use linkrs_wire::fulltext::ClearFulltextIndexRequest;
        let url = format!("{}/fulltext/indexes/clear", self.base_url);
        let request = ClearFulltextIndexRequest {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: field_name.to_string(),
            force,
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        let response = Self::check_response(response, "Failed to clear fulltext index").await?;
        Ok(response.json().await?)
    }

    // ── Vector search management ──

    /// Create a vector index via `POST /v1/vector/indexes`.
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/vector/indexes", self.base_url);
        let body = serde_json::json!({
            "space_id": space_id,
            "tag_name": tag_name,
            "field_name": field_name,
            "vector_size": vector_size,
        });
        let response = self
            .authed(self.inner.post(&url))
            .json(&body)
            .send()
            .await?;
        let response = Self::check_response(response, "Failed to create vector index").await?;
        Ok(response.json().await?)
    }

    /// List vector indexes via `GET /v1/vector/indexes`.
    pub async fn list_vector_indexes(&self) -> Result<serde_json::Value> {
        let url = format!("{}/vector/indexes", self.base_url);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to list vector indexes").await?;
        Ok(response.json().await?)
    }

    /// Get one vector index via
    /// `GET /v1/vector/indexes/{space_id}/{tag_name}/{field_name}`.
    pub async fn get_vector_index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/vector/indexes/{}/{}/{}",
            self.base_url, space_id, tag_name, field_name
        );
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get vector index").await?;
        Ok(response.json().await?)
    }

    /// Drop a vector index via
    /// `DELETE /v1/vector/indexes/{space_id}/{tag_name}/{field_name}`.
    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/vector/indexes/{}/{}/{}",
            self.base_url, space_id, tag_name, field_name
        );
        let response = self.authed(self.inner.delete(&url)).send().await?;
        Self::check_response(response, "Failed to drop vector index").await?;
        Ok(())
    }

    /// Vector similarity search via `POST /v1/vector/search`.
    pub async fn vector_search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/vector/search", self.base_url);
        let body = serde_json::json!({
            "space_id": space_id,
            "tag_name": tag_name,
            "field_name": field_name,
            "query_vector": query_vector,
            "limit": limit,
        });
        let response = self
            .authed(self.inner.post(&url))
            .json(&body)
            .send()
            .await?;
        let response = Self::check_response(response, "Vector search failed").await?;
        Ok(response.json().await?)
    }

    /// Count vectors of one index via
    /// `GET /v1/vector/{space_id}/{tag_name}/{field_name}/count`.
    pub async fn vector_count(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/vector/{}/{}/{}/count",
            self.base_url, space_id, tag_name, field_name
        );
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to count vectors").await?;
        Ok(response.json().await?)
    }

    /// Start an async vector index rebuild via
    /// `POST /v1/vector/indexes/rebuild`.
    pub async fn rebuild_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<serde_json::Value> {
        use linkrs_wire::vector::RebuildVectorIndexRequest;
        let url = format!("{}/vector/indexes/rebuild", self.base_url);
        let request = RebuildVectorIndexRequest {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: field_name.to_string(),
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        let response = Self::check_response(response, "Failed to start vector rebuild").await?;
        Ok(response.json().await?)
    }

    /// Poll a vector rebuild via `GET /v1/vector/rebuilds/{id}`.
    pub async fn vector_rebuild_status(&self, rebuild_id: &str) -> Result<serde_json::Value> {
        let url = format!("{}/vector/rebuilds/{}", self.base_url, rebuild_id);
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get rebuild status").await?;
        Ok(response.json().await?)
    }

    /// Clear all vectors of one index via
    /// `POST /v1/vector/indexes/clear` (destructive).
    pub async fn clear_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> Result<serde_json::Value> {
        use linkrs_wire::vector::ClearVectorIndexRequest;
        let url = format!("{}/vector/indexes/clear", self.base_url);
        let request = ClearVectorIndexRequest {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: field_name.to_string(),
            force,
        };
        let response = self
            .authed(self.inner.post(&url))
            .json(&request)
            .send()
            .await?;
        let response = Self::check_response(response, "Failed to clear vector index").await?;
        Ok(response.json().await?)
    }

    /// Get one vector point by id via
    /// `GET /v1/vector/{space_id}/{tag_name}/{field_name}/{point_id}`.
    pub async fn get_vector_point(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_id: &str,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/vector/{}/{}/{}/{}",
            self.base_url, space_id, tag_name, field_name, point_id
        );
        let response = self.authed(self.inner.get(&url)).send().await?;
        let response = Self::check_response(response, "Failed to get vector point").await?;
        Ok(response.json().await?)
    }

    /// Scroll vector points via `POST /v1/vector/scroll`.
    ///
    /// `offset` resumes a previous page; payload/vector inclusion is left to
    /// the server defaults by omitting those keys.
    pub async fn scroll_vectors(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        limit: usize,
        offset: Option<&str>,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/vector/scroll", self.base_url);
        let mut body = serde_json::json!({
            "space_id": space_id,
            "tag_name": tag_name,
            "field_name": field_name,
            "limit": limit,
        });
        if let Some(offset) = offset {
            body["offset"] = serde_json::Value::String(offset.to_string());
        }
        let response = self
            .authed(self.inner.post(&url))
            .json(&body)
            .send()
            .await?;
        let response = Self::check_response(response, "Vector scroll failed").await?;
        Ok(response.json().await?)
    }

    /// Overwrite payload for vector points via `PUT /v1/vector/payload`.
    pub async fn set_vector_payload(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: &[String],
        payload: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/vector/payload", self.base_url);
        let body = serde_json::json!({
            "space_id": space_id,
            "tag_name": tag_name,
            "field_name": field_name,
            "point_ids": point_ids,
            "payload": payload,
        });
        let response = self.authed(self.inner.put(&url)).json(&body).send().await?;
        let response = Self::check_response(response, "Failed to set vector payload").await?;
        Ok(response.json().await?)
    }

    /// Merge payload fields for vector points via `PUT /v1/vector/payload/fields`.
    pub async fn merge_vector_payload_fields(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: &[String],
        payload: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/vector/payload/fields", self.base_url);
        let body = serde_json::json!({
            "space_id": space_id,
            "tag_name": tag_name,
            "field_name": field_name,
            "point_ids": point_ids,
            "payload": payload,
        });
        let response = self.authed(self.inner.put(&url)).json(&body).send().await?;
        let response = Self::check_response(response, "Failed to merge vector payload").await?;
        Ok(response.json().await?)
    }

    /// Delete payload keys from vector points via `POST /v1/vector/payload/delete`.
    pub async fn delete_vector_payload(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_ids: &[String],
        keys: &[String],
    ) -> Result<serde_json::Value> {
        let url = format!("{}/vector/payload/delete", self.base_url);
        let body = serde_json::json!({
            "space_id": space_id,
            "tag_name": tag_name,
            "field_name": field_name,
            "point_ids": point_ids,
            "keys": keys,
        });
        let response = self
            .authed(self.inner.post(&url))
            .json(&body)
            .send()
            .await?;
        let response = Self::check_response(response, "Failed to delete vector payload").await?;
        Ok(response.json().await?)
    }

    // ── Auth ──

    /// Login and authenticate (low-level API, public route without session header)
    async fn login(&self, username: &str, password: &str) -> Result<LoginResponse> {
        let url = format!("{}/auth/login", self.base_url);
        let request = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        let response = Self::check_response(response, "Login failed").await?;

        let login_resp: LoginResponse = response.json().await?;
        Ok(login_resp)
    }
}
