//! HTTP client for connecting to GraphDB server

use std::time::Duration;

use crate::client::batch::{BatchError, BatchItem, BatchResult, BatchStatus, BatchType};
use crate::client::config::{ClientConfig, SessionInfo};
use crate::client::config_types::ServerConfigResponse;
use crate::client::request_types::*;
use crate::client::response_types::*;
use crate::client::schema::PropertyDef;
use crate::client::stats::{DatabaseStatistics, QueryStatistics, SessionStatistics};
use crate::client::transaction::{TransactionInfo, TransactionOptions};
use crate::client::types::{EdgeTypeInfo, QueryResult, SpaceInfo, TagInfo};
use crate::client::validation::{ValidationError, ValidationResult, ValidationWarning};
use crate::client::vector::{VectorMatch, VectorSearchResult};
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

        let query_resp: QueryResponse = response.json().await?;

        if !query_resp.success {
            let err = query_resp.error.unwrap_or(QueryError {
                code: "UNKNOWN".to_string(),
                message: "Unknown error".to_string(),
            });
            return Err(CliError::query(format!("{}: {}", err.code, err.message)));
        }

        let data = query_resp.data.unwrap_or(QueryData {
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
        });

        let metadata = query_resp.metadata.unwrap_or(QueryMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
        });

        Ok(QueryResult {
            columns: data.columns,
            rows: data.rows,
            row_count: data.row_count,
            execution_time_ms: metadata.execution_time_ms,
            rows_scanned: metadata.rows_scanned,
            error: None,
        })
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

    /// Begin a new transaction
    pub async fn begin_transaction(&self, options: TransactionOptions) -> Result<TransactionInfo> {
        let url = format!("{}/transactions", self.base_url);

        let session_id = self
            .session_info
            .as_ref()
            .map(|s| s.session_id)
            .ok_or_else(|| CliError::session("Not connected".to_string()))?;

        let request = BeginTransactionRequest {
            session_id,
            read_only: options.read_only,
            timeout_seconds: options.timeout_seconds,
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

        let txn_resp: TransactionResponse = response.json().await?;
        Ok(TransactionInfo {
            transaction_id: txn_resp.transaction_id,
            status: txn_resp.status,
        })
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

        let props: Vec<PropertyDefInput> = properties
            .into_iter()
            .map(|p| PropertyDefInput {
                name: p.name,
                data_type: p.data_type.to_string(),
                nullable: p.nullable,
            })
            .collect();

        let request = CreateTagRequest {
            name: name.to_string(),
            properties: props,
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

        let props: Vec<PropertyDefInput> = properties
            .into_iter()
            .map(|p| PropertyDefInput {
                name: p.name,
                data_type: p.data_type.to_string(),
                nullable: p.nullable,
            })
            .collect();

        let request = CreateEdgeTypeRequest {
            name: name.to_string(),
            properties: props,
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

    /// Create a batch task
    pub async fn create_batch(
        &self,
        space_id: u64,
        batch_type: BatchType,
        batch_size: usize,
    ) -> Result<String> {
        let url = format!("{}/batch", self.base_url);

        let batch_type_str = match batch_type {
            BatchType::Vertex => "vertex",
            BatchType::Edge => "edge",
            BatchType::Mixed => "mixed",
        };

        let request = CreateBatchRequest {
            space_id,
            batch_type: batch_type_str.to_string(),
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

        let batch_items: Vec<crate::client::request_types::BatchItem> = items
            .into_iter()
            .map(|item| match item {
                BatchItem::Vertex(v) => {
                    crate::client::request_types::BatchItem::Vertex(VertexData {
                        vid: v.vid,
                        tags: v.tags,
                        properties: v.properties,
                    })
                }
                BatchItem::Edge(e) => crate::client::request_types::BatchItem::Edge(EdgeData {
                    edge_type: e.edge_type,
                    src_vid: e.src_vid,
                    dst_vid: e.dst_vid,
                    properties: e.properties,
                }),
            })
            .collect();

        let request = AddBatchItemsRequest { items: batch_items };

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
    pub async fn execute_batch(&self, batch_id: &str) -> Result<BatchResult> {
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

        let exec_resp: ExecuteBatchResponse = response.json().await?;
        Ok(BatchResult {
            batch_id: exec_resp.batch_id,
            status: format!("{:?}", exec_resp.status),
            vertices_inserted: exec_resp.result.vertices_inserted,
            edges_inserted: exec_resp.result.edges_inserted,
            errors: exec_resp
                .result
                .errors
                .into_iter()
                .map(|e| BatchError {
                    index: e.index,
                    item_type: format!("{:?}", e.item_type),
                    error: e.error,
                })
                .collect(),
        })
    }

    /// Get batch status
    pub async fn get_batch_status(&self, batch_id: &str) -> Result<BatchStatus> {
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

        let status_resp: BatchStatusResponse = response.json().await?;
        Ok(BatchStatus {
            batch_id: status_resp.batch_id,
            status: format!("{:?}", status_resp.status),
            total: status_resp.progress.total,
            processed: status_resp.progress.processed,
            succeeded: status_resp.progress.succeeded,
            failed: status_resp.progress.failed,
        })
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

        let stats: SessionStatistics = response.json().await?;
        Ok(stats)
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

        let stats: QueryStatistics = response.json().await?;
        Ok(stats)
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

        let stats: DatabaseStatistics = response.json().await?;
        Ok(stats)
    }

    /// Validate a query without executing it
    pub async fn validate_query(&self, query: &str) -> Result<ValidationResult> {
        let url = format!("{}/query/validate", self.base_url);

        let session_id = self.session_info.as_ref().map(|s| s.session_id);

        let request = ValidateQueryRequest {
            query: query.to_string(),
            session_id,
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to validate query ({}): {}",
                status, body
            )));
        }

        let validate_resp: ValidateQueryResponse = response.json().await?;
        Ok(ValidationResult {
            valid: validate_resp.valid,
            errors: validate_resp
                .errors
                .into_iter()
                .map(|e| ValidationError {
                    code: e.code,
                    message: e.message,
                    position: e.position,
                    line: e.line,
                    column: e.column,
                })
                .collect(),
            warnings: validate_resp
                .warnings
                .into_iter()
                .map(|w| ValidationWarning {
                    code: w.code,
                    message: w.message,
                    suggestion: w.suggestion,
                })
                .collect(),
            estimated_cost: validate_resp.estimated_cost,
        })
    }

    /// Get server configuration
    pub async fn get_config(&self) -> Result<crate::client::config_types::ServerConfig> {
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

        let config_resp: ServerConfigResponse = response.json().await?;
        Ok(crate::client::config_types::ServerConfig {
            version: config_resp.version,
            sections: config_resp
                .sections
                .into_iter()
                .map(|s| crate::client::config_types::ConfigSection {
                    name: s.name,
                    description: s.description,
                    items: s
                        .items
                        .into_iter()
                        .map(|i| crate::client::config_types::ConfigItem {
                            key: i.key,
                            value: i.value,
                            default_value: i.default_value,
                            description: i.description,
                            mutable: i.mutable,
                        })
                        .collect(),
                })
                .collect(),
        })
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

    /// Create a vector index
    pub async fn create_vector_index(
        &self,
        space: &str,
        name: &str,
        tag: &str,
        field: &str,
        dimension: usize,
        metric: &str,
    ) -> Result<()> {
        let url = format!("{}/schema/spaces/{}/vector-indexes", self.base_url, space);

        let request = CreateVectorIndexRequest {
            name: name.to_string(),
            tag: tag.to_string(),
            field: field.to_string(),
            dimension,
            metric: metric.to_string(),
        };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to create vector index ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    /// Drop a vector index
    pub async fn drop_vector_index(&self, space: &str, name: &str) -> Result<()> {
        let url = format!(
            "{}/schema/spaces/{}/vector-indexes/{}",
            self.base_url, space, name
        );

        let response = self.inner.delete(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to drop vector index ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    /// Search similar vectors
    pub async fn vector_search(
        &self,
        space: &str,
        index_name: &str,
        vector: Vec<f32>,
        top_k: usize,
    ) -> Result<VectorSearchResult> {
        let url = format!(
            "{}/schema/spaces/{}/vector-indexes/{}/search",
            self.base_url, space, index_name
        );

        let request = VectorSearchRequest { vector, top_k };

        let response = self.inner.post(&url).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CliError::query(format!(
                "Failed to search vectors ({}): {}",
                status, body
            )));
        }

        let search_resp: VectorSearchResponse = response.json().await?;

        let results = search_resp
            .results
            .into_iter()
            .map(|r| VectorMatch {
                vid: r.vid,
                score: r.score,
                properties: r.properties,
            })
            .collect();

        Ok(VectorSearchResult {
            total: search_resp.total,
            results,
        })
    }

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
