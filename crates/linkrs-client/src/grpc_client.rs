//! Typed gRPC client for the Linkrs service.
//!
//! [`GrpcClient`] wraps a tonic channel plus the generated
//! `LinkrsServiceClient`, so callers issue typed RPCs without hand-writing
//! tonic glue. The generated request/response message types are re-exported
//! under [`proto`]; methods take the generated request messages and return the
//! generated response messages directly (requests without fields take no
//! argument).
//!
//! Covered RPC groups: health, authentication, sessions (including session
//! variables), queries (including cursors), transactions (including savepoints
//! and inspection), schema, configuration, statistics, batch, migration, and
//! import/export. User management, custom functions, vector, fulltext, and
//! sync RPC groups are deliberately out of scope for now.
//!
//! Authentication mirrors the HTTP client: [`GrpcClient::login`] stores the
//! session id returned by the server behind a lock, [`GrpcClient::logout`] and
//! [`GrpcClient::auth_me`] reuse it, and every outgoing request carries it as
//! `x-session-id` metadata through [`GrpcClient::authed`]. The service
//! contract additionally accepts the session id inside request messages where
//! such a field exists; request-carrying methods leave those fields to the
//! caller.
//!
//! Streaming RPCs ([`GrpcClient::execute_query_stream`],
//! [`GrpcClient::stream_migration_progress`]) hand back the raw
//! `tonic::Streaming` values so callers control consumption and backpressure.
//!
//! Errors are mapped into the crate [`ClientError`]: transport failures become
//! `ConnectionError`, non-OK statuses become `GrpcError`.

use std::sync::RwLock;
use std::time::Duration;

use tonic::transport::{Channel, Endpoint};

use crate::config::ClientConfig;
use crate::utils::error::{ClientError, Result};

/// Generated protobuf message types for the Linkrs service contract.
pub mod proto {
    tonic::include_proto!("linkrs");
}

use proto::linkrs_service_client::LinkrsServiceClient;
use proto::*;

/// Request metadata key carrying the session id issued by `Login`.
const SESSION_METADATA_KEY: &str = "x-session-id";

/// Typed gRPC client for a Linkrs server.
///
/// Cheap to construct; the underlying channel reconnects lazily when built via
/// [`GrpcClient::new`] / [`GrpcClient::with_config`], while
/// [`GrpcClient::connect`] verifies reachability up front.
pub struct GrpcClient {
    inner: LinkrsServiceClient<Channel>,
    endpoint: String,
    session: RwLock<Option<String>>,
}

impl GrpcClient {
    /// Create a client for `host:port` with default settings (lazy channel).
    pub fn new(host: &str, port: u16) -> Result<Self> {
        Self::with_config(ClientConfig::new().with_host(host).with_port(port))
    }

    /// Create a client from a [`ClientConfig`] (lazy channel). The configured
    /// timeout bounds both connection establishment and individual calls.
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        let endpoint = endpoint_address(&config.host, config.port);
        let channel = build_endpoint(&endpoint, config.timeout_seconds)?.connect_lazy();
        Ok(Self::from_channel(channel, endpoint))
    }

    /// Connect to `host:port` eagerly, failing if the server is unreachable.
    pub async fn connect(host: &str, port: u16) -> Result<Self> {
        let config = ClientConfig::new().with_host(host).with_port(port);
        let endpoint = endpoint_address(&config.host, config.port);
        let channel = build_endpoint(&endpoint, config.timeout_seconds)?
            .connect()
            .await
            .map_err(|e| {
                ClientError::connection(format!(
                    "Failed to connect to gRPC endpoint {}: {}",
                    endpoint, e
                ))
            })?;
        Ok(Self::from_channel(channel, endpoint))
    }

    fn from_channel(channel: Channel, endpoint: String) -> Self {
        Self {
            inner: LinkrsServiceClient::new(channel),
            endpoint,
            session: RwLock::new(None),
        }
    }

    /// Endpoint address the client talks to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Session id issued by the last successful [`GrpcClient::login`].
    pub fn session_id(&self) -> Option<i64> {
        self.stored_session().and_then(|id| id.parse().ok())
    }

    /// Attach the session metadata required by protected RPCs.
    pub fn authed<T>(&self, mut request: tonic::Request<T>) -> tonic::Request<T> {
        if let Some(session) = self.stored_session() {
            if let (Ok(key), Ok(value)) = (
                tonic::metadata::MetadataKey::from_bytes(SESSION_METADATA_KEY.as_bytes()),
                tonic::metadata::MetadataValue::<tonic::metadata::Ascii>::try_from(
                    session.as_str(),
                ),
            ) {
                request.metadata_mut().insert(key, value);
            }
        }
        request
    }

    /// Drop the stored session id so it does not linger after logout.
    pub fn clear_credentials(&self) {
        *self.session.write().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn stored_session(&self) -> Option<String> {
        self.session
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn store_session(&self, session_id: String) {
        *self.session.write().unwrap_or_else(|e| e.into_inner()) = Some(session_id);
    }

    fn require_session(&self) -> Result<String> {
        self.stored_session()
            .ok_or_else(|| ClientError::auth("no active gRPC session; call login() first"))
    }

    // ================================================================
    // Health
    // ================================================================

    /// Check server health, version, and uptime.
    pub async fn health_check(&self) -> Result<HealthCheckResponse> {
        let mut client = self.inner.clone();
        client
            .health_check(self.authed(tonic::Request::new(HealthCheckRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Authentication
    // ================================================================

    /// Authenticate and remember the returned session id for later calls.
    pub async fn login(&self, username: &str, password: &str) -> Result<LoginResponse> {
        let request = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            space: None,
        };
        let mut client = self.inner.clone();
        let response = client
            .login(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)?;
        if response.success {
            self.store_session(response.session_id.clone());
        }
        Ok(response)
    }

    /// End the remembered session and drop the stored session id.
    pub async fn logout(&self) -> Result<LogoutResponse> {
        let session_id = self.require_session()?;
        let mut client = self.inner.clone();
        let result = client
            .logout(self.authed(tonic::Request::new(LogoutRequest { session_id })))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error);
        self.clear_credentials();
        result
    }

    /// Fetch identity information for the remembered session.
    pub async fn auth_me(&self) -> Result<AuthMeResponse> {
        let session_id = self.require_session()?;
        let mut client = self.inner.clone();
        client
            .auth_me(self.authed(tonic::Request::new(AuthMeRequest { session_id })))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Sessions
    // ================================================================

    /// Create a server-side session.
    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        let mut client = self.inner.clone();
        client
            .create_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Inspect a server-side session.
    pub async fn get_session(&self, request: GetSessionRequest) -> Result<GetSessionResponse> {
        let mut client = self.inner.clone();
        client
            .get_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Close a server-side session.
    pub async fn close_session(
        &self,
        request: CloseSessionRequest,
    ) -> Result<CloseSessionResponse> {
        let mut client = self.inner.clone();
        client
            .close_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List active server-side sessions.
    pub async fn list_sessions(
        &self,
        request: ListSessionsRequest,
    ) -> Result<ListSessionsResponse> {
        let mut client = self.inner.clone();
        client
            .list_sessions(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Terminate another session as an administrator.
    pub async fn kill_session(&self, request: KillSessionRequest) -> Result<KillSessionResponse> {
        let mut client = self.inner.clone();
        client
            .kill_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List variables of a session.
    pub async fn list_session_variables(
        &self,
        request: ListSessionVariablesRequest,
    ) -> Result<ListSessionVariablesResponse> {
        let mut client = self.inner.clone();
        client
            .list_session_variables(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Set a session variable.
    pub async fn set_session_variable(
        &self,
        request: SetSessionVariableRequest,
    ) -> Result<SetSessionVariableResponse> {
        let mut client = self.inner.clone();
        client
            .set_session_variable(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Delete a session variable.
    pub async fn delete_session_variable(
        &self,
        request: DeleteSessionVariableRequest,
    ) -> Result<DeleteSessionVariableResponse> {
        let mut client = self.inner.clone();
        client
            .delete_session_variable(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Queries
    // ================================================================

    /// Execute a query (parameters travel inside the request message).
    pub async fn execute_query(
        &self,
        request: ExecuteQueryRequest,
    ) -> Result<ExecuteQueryResponse> {
        let mut client = self.inner.clone();
        client
            .execute_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Validate a query without executing it.
    pub async fn validate_query(
        &self,
        request: ValidateQueryRequest,
    ) -> Result<ValidateQueryResponse> {
        let mut client = self.inner.clone();
        client
            .validate_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Return the query plan for a query.
    pub async fn explain_query(
        &self,
        request: ExplainQueryRequest,
    ) -> Result<ExplainQueryResponse> {
        let mut client = self.inner.clone();
        client
            .explain_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Execute a query and stream result chunks.
    ///
    /// Returns the raw server stream: consume it with `tonic::Streaming::message`
    /// until it yields `Ok(None)`.
    pub async fn execute_query_stream(
        &self,
        request: ExecuteQueryRequest,
    ) -> Result<tonic::Streaming<StreamResponse>> {
        let mut client = self.inner.clone();
        client
            .execute_query_stream(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List currently running queries.
    pub async fn list_queries(&self) -> Result<ListQueriesResponse> {
        let mut client = self.inner.clone();
        client
            .list_queries(self.authed(tonic::Request::new(ListQueriesRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Kill a running query.
    pub async fn kill_query(&self, request: KillQueryRequest) -> Result<KillQueryResponse> {
        let mut client = self.inner.clone();
        client
            .kill_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Open a cursor for paged query result retrieval.
    pub async fn open_cursor(&self, request: OpenCursorRequest) -> Result<OpenCursorResponse> {
        let mut client = self.inner.clone();
        client
            .open_cursor(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch the next page of a cursor.
    pub async fn fetch_cursor(&self, request: FetchCursorRequest) -> Result<FetchCursorResponse> {
        let mut client = self.inner.clone();
        client
            .fetch_cursor(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Close a cursor and release its server-side resources.
    pub async fn close_cursor(&self, request: CloseCursorRequest) -> Result<CloseCursorResponse> {
        let mut client = self.inner.clone();
        client
            .close_cursor(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Transactions
    // ================================================================

    /// Begin a transaction.
    pub async fn begin_transaction(
        &self,
        request: BeginTransactionRequest,
    ) -> Result<BeginTransactionResponse> {
        let mut client = self.inner.clone();
        client
            .begin_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Commit a transaction.
    pub async fn commit_transaction(
        &self,
        request: CommitTransactionRequest,
    ) -> Result<CommitTransactionResponse> {
        let mut client = self.inner.clone();
        client
            .commit_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back a transaction.
    pub async fn rollback_transaction(
        &self,
        request: RollbackTransactionRequest,
    ) -> Result<RollbackTransactionResponse> {
        let mut client = self.inner.clone();
        client
            .rollback_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create a savepoint inside a transaction.
    pub async fn create_savepoint(
        &self,
        request: CreateSavepointRequest,
    ) -> Result<CreateSavepointResponse> {
        let mut client = self.inner.clone();
        client
            .create_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back to a savepoint.
    pub async fn rollback_to_savepoint(
        &self,
        request: RollbackToSavepointRequest,
    ) -> Result<RollbackToSavepointResponse> {
        let mut client = self.inner.clone();
        client
            .rollback_to_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Release a savepoint.
    pub async fn release_savepoint(
        &self,
        request: ReleaseSavepointRequest,
    ) -> Result<ReleaseSavepointResponse> {
        let mut client = self.inner.clone();
        client
            .release_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List active transactions.
    pub async fn list_transactions(&self) -> Result<ListTransactionsResponse> {
        let mut client = self.inner.clone();
        client
            .list_transactions(self.authed(tonic::Request::new(ListTransactionsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Kill a transaction.
    pub async fn kill_transaction(
        &self,
        request: KillTransactionRequest,
    ) -> Result<KillTransactionResponse> {
        let mut client = self.inner.clone();
        client
            .kill_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch transaction manager metrics.
    pub async fn transaction_metrics(&self) -> Result<GetTransactionMetricsResponse> {
        let mut client = self.inner.clone();
        client
            .get_transaction_metrics(
                self.authed(tonic::Request::new(GetTransactionMetricsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Schema
    // ================================================================

    /// Create a space.
    pub async fn create_space(&self, request: CreateSpaceRequest) -> Result<CreateSpaceResponse> {
        let mut client = self.inner.clone();
        client
            .create_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch a space by name.
    pub async fn get_space(&self, request: GetSpaceRequest) -> Result<GetSpaceResponse> {
        let mut client = self.inner.clone();
        client
            .get_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop a space.
    pub async fn drop_space(&self, request: DropSpaceRequest) -> Result<DropSpaceResponse> {
        let mut client = self.inner.clone();
        client
            .drop_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List spaces.
    pub async fn list_spaces(&self) -> Result<ListSpacesResponse> {
        let mut client = self.inner.clone();
        client
            .list_spaces(self.authed(tonic::Request::new(ListSpacesRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create a tag.
    pub async fn create_tag(&self, request: CreateTagRequest) -> Result<CreateTagResponse> {
        let mut client = self.inner.clone();
        client
            .create_tag(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch a tag definition.
    pub async fn get_tag(&self, request: GetTagRequest) -> Result<GetTagResponse> {
        let mut client = self.inner.clone();
        client
            .get_tag(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List tags in a space.
    pub async fn list_tags(&self, request: ListTagsRequest) -> Result<ListTagsResponse> {
        let mut client = self.inner.clone();
        client
            .list_tags(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop a tag.
    pub async fn drop_tag(&self, request: DropTagRequest) -> Result<DropTagResponse> {
        let mut client = self.inner.clone();
        client
            .drop_tag(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create an edge type.
    pub async fn create_edge_type(
        &self,
        request: CreateEdgeTypeRequest,
    ) -> Result<CreateEdgeTypeResponse> {
        let mut client = self.inner.clone();
        client
            .create_edge_type(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch an edge type definition.
    pub async fn get_edge_type(&self, request: GetEdgeTypeRequest) -> Result<GetEdgeTypeResponse> {
        let mut client = self.inner.clone();
        client
            .get_edge_type(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List edge types in a space.
    pub async fn list_edge_types(
        &self,
        request: ListEdgeTypesRequest,
    ) -> Result<ListEdgeTypesResponse> {
        let mut client = self.inner.clone();
        client
            .list_edge_types(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop an edge type.
    pub async fn drop_edge_type(
        &self,
        request: DropEdgeTypeRequest,
    ) -> Result<DropEdgeTypeResponse> {
        let mut client = self.inner.clone();
        client
            .drop_edge_type(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Configuration
    // ================================================================

    /// Fetch the full server configuration.
    pub async fn get_config(&self) -> Result<GetConfigResponse> {
        let mut client = self.inner.clone();
        client
            .get_config(self.authed(tonic::Request::new(GetConfigRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch a single configuration key.
    pub async fn get_config_key(
        &self,
        request: GetConfigKeyRequest,
    ) -> Result<GetConfigKeyResponse> {
        let mut client = self.inner.clone();
        client
            .get_config_key(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Update a configuration key.
    pub async fn update_config(
        &self,
        request: UpdateConfigRequest,
    ) -> Result<UpdateConfigResponse> {
        let mut client = self.inner.clone();
        client
            .update_config(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Reset a configuration key to its default.
    pub async fn reset_config(&self, request: ResetConfigRequest) -> Result<ResetConfigResponse> {
        let mut client = self.inner.clone();
        client
            .reset_config(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Update several configuration keys at once.
    pub async fn bulk_update_config(
        &self,
        request: BulkUpdateConfigRequest,
    ) -> Result<BulkUpdateConfigResponse> {
        let mut client = self.inner.clone();
        client
            .bulk_update_config(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Statistics
    // ================================================================

    /// Fetch session statistics.
    pub async fn get_session_statistics(
        &self,
        request: GetSessionStatisticsRequest,
    ) -> Result<GetSessionStatisticsResponse> {
        let mut client = self.inner.clone();
        client
            .get_session_statistics(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch query statistics.
    pub async fn get_query_statistics(
        &self,
        request: GetQueryStatisticsRequest,
    ) -> Result<GetQueryStatisticsResponse> {
        let mut client = self.inner.clone();
        client
            .get_query_statistics(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch database statistics.
    pub async fn get_database_statistics(&self) -> Result<GetDatabaseStatisticsResponse> {
        let mut client = self.inner.clone();
        client
            .get_database_statistics(
                self.authed(tonic::Request::new(GetDatabaseStatisticsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch system statistics.
    pub async fn get_system_statistics(&self) -> Result<GetSystemStatisticsResponse> {
        let mut client = self.inner.clone();
        client
            .get_system_statistics(self.authed(tonic::Request::new(GetSystemStatisticsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Batch
    // ================================================================

    /// Create a batch job.
    pub async fn create_batch(&self, request: CreateBatchRequest) -> Result<CreateBatchResponse> {
        let mut client = self.inner.clone();
        client
            .create_batch(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Append items to a batch job.
    pub async fn add_batch_items(
        &self,
        request: AddBatchItemsRequest,
    ) -> Result<AddBatchItemsResponse> {
        let mut client = self.inner.clone();
        client
            .add_batch_items(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Execute a batch job.
    pub async fn execute_batch(
        &self,
        request: ExecuteBatchRequest,
    ) -> Result<ExecuteBatchResponse> {
        let mut client = self.inner.clone();
        client
            .execute_batch(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch batch job status.
    pub async fn get_batch_status(
        &self,
        request: GetBatchStatusRequest,
    ) -> Result<GetBatchStatusResponse> {
        let mut client = self.inner.clone();
        client
            .get_batch_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Cancel a batch job.
    pub async fn cancel_batch(&self, request: CancelBatchRequest) -> Result<CancelBatchResponse> {
        let mut client = self.inner.clone();
        client
            .cancel_batch(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Migration
    // ================================================================

    /// Plan a schema migration between two schema versions.
    pub async fn create_migration_plan(
        &self,
        request: MigratePlanRequest,
    ) -> Result<MigratePlanResponse> {
        let mut client = self.inner.clone();
        client
            .migrate_plan(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Execute a previously planned migration.
    pub async fn execute_migration(
        &self,
        request: MigrateExecuteRequest,
    ) -> Result<MigrateExecuteResponse> {
        let mut client = self.inner.clone();
        client
            .migrate_execute(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back an executed migration.
    pub async fn rollback_migration(
        &self,
        request: MigrateRollbackRequest,
    ) -> Result<MigrateRollbackResponse> {
        let mut client = self.inner.clone();
        client
            .migrate_rollback(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch the schema version history of a tag or edge type.
    pub async fn get_schema_version_history(
        &self,
        request: VersionHistoryRequest,
    ) -> Result<VersionHistoryResponse> {
        let mut client = self.inner.clone();
        client
            .get_version_history(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Stream migration progress events.
    ///
    /// Returns the raw server stream: consume it with `tonic::Streaming::message`
    /// until it yields `Ok(None)`.
    pub async fn stream_migration_progress(
        &self,
        request: StreamMigrationProgressRequest,
    ) -> Result<tonic::Streaming<MigrationProgressEvent>> {
        let mut client = self.inner.clone();
        client
            .stream_migration_progress(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Import / Export
    // ================================================================

    /// Import data into a space.
    pub async fn import_data(&self, request: ImportDataRequest) -> Result<ImportDataResponse> {
        let mut client = self.inner.clone();
        client
            .import_data(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch the status of an import job.
    pub async fn get_import_status(
        &self,
        request: GetImportStatusRequest,
    ) -> Result<GetImportStatusResponse> {
        let mut client = self.inner.clone();
        client
            .get_import_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Export query results in the requested format.
    pub async fn export_data(&self, request: ExportDataRequest) -> Result<ExportDataResponse> {
        let mut client = self.inner.clone();
        client
            .export_data(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}

fn endpoint_address(host: &str, port: u16) -> String {
    format!("http://{}:{}", host, port)
}

fn build_endpoint(address: &str, timeout_seconds: u64) -> Result<Endpoint> {
    let endpoint = Endpoint::from_shared(address.to_string())
        .map_err(|e| ClientError::config(format!("Invalid gRPC endpoint {}: {}", address, e)))?;
    if timeout_seconds == 0 {
        return Ok(endpoint);
    }
    Ok(endpoint
        .timeout(Duration::from_secs(timeout_seconds))
        .connect_timeout(Duration::from_secs(timeout_seconds)))
}

fn status_error(status: tonic::Status) -> ClientError {
    ClientError::grpc(format!("{:?}: {}", status.code(), status.message()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> GrpcClient {
        GrpcClient::new("127.0.0.1", 7070).expect("client should build")
    }

    #[tokio::test]
    async fn endpoint_address_is_built_from_host_and_port() {
        let client = test_client();
        assert_eq!(client.endpoint(), "http://127.0.0.1:7070");
        assert!(client.session_id().is_none());
    }

    #[tokio::test]
    async fn authed_without_session_omits_session_metadata() {
        let client = test_client();
        let request = client.authed(tonic::Request::new(HealthCheckRequest {}));
        assert!(request.metadata().get(SESSION_METADATA_KEY).is_none());
    }

    #[tokio::test]
    async fn authed_injects_stored_session_metadata() {
        let client = test_client();
        client.store_session("42".to_string());
        assert_eq!(client.session_id(), Some(42));

        let request = client.authed(tonic::Request::new(HealthCheckRequest {}));
        let value = request
            .metadata()
            .get(SESSION_METADATA_KEY)
            .map(|value| value.to_str().unwrap_or_default().to_string());
        assert_eq!(value.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn session_id_is_none_for_non_numeric_session() {
        let client = test_client();
        client.store_session("not-a-number".to_string());
        assert!(client.session_id().is_none());
    }

    #[tokio::test]
    async fn clear_credentials_drops_stored_session() {
        let client = test_client();
        client.store_session("42".to_string());
        client.clear_credentials();
        assert!(client.session_id().is_none());

        let request = client.authed(tonic::Request::new(HealthCheckRequest {}));
        assert!(request.metadata().get(SESSION_METADATA_KEY).is_none());
    }

    #[test]
    fn status_error_preserves_code_and_message() {
        let error = status_error(tonic::Status::unauthenticated("bad credentials"));
        match error {
            ClientError::GrpcError(message) => {
                assert!(message.contains("Unauthenticated"), "message: {}", message);
                assert!(message.contains("bad credentials"), "message: {}", message);
            }
            other => panic!("expected grpc error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn logout_without_session_is_an_auth_error() {
        let client = test_client();
        let result = client.logout().await;
        assert!(
            matches!(&result, Err(ClientError::AuthError(_))),
            "expected auth error, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn auth_me_without_session_is_an_auth_error() {
        let client = test_client();
        let result = client.auth_me().await;
        assert!(
            matches!(&result, Err(ClientError::AuthError(_))),
            "expected auth error, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn calls_to_unreachable_endpoint_return_error() {
        let config = ClientConfig::new()
            .with_host("127.0.0.1")
            .with_port(1)
            .with_timeout(2);
        let client = GrpcClient::with_config(config).expect("client should build");
        let result = client.health_check().await;
        assert!(result.is_err(), "expected transport error");
    }
}
