//! Typed gRPC client for the Linkrs service.
//!
//! [`GrpcClient`] wraps a tonic channel plus the generated
//! `LinkrsServiceClient`, so callers issue typed RPCs without hand-writing
//! tonic glue. The generated request/response message types are re-exported
//! under [`proto`]; methods take the generated request messages and return the
//! generated response messages directly (requests without fields take no
//! argument).
//!
//! Every RPC in `proto/linkrs.proto` is wrapped: health, authentication,
//! sessions, user management, queries (including cursors), transactions,
//! schema, configuration, statistics, batch, migration, import/export,
//! custom functions, vector, fulltext, and sync.
//!
//! Authentication mirrors the HTTP client: [`GrpcClient::login`] stores the
//! session id returned by the server behind a lock, [`GrpcClient::logout`] and
//! [`GrpcClient::auth_me`] reuse it, and [`GrpcClient::authed`] additionally
//! sends it as `x-session-id` metadata for servers that install an auth
//! interceptor. The current server reads the session id from request messages
//! instead, so request-carrying methods fill missing `session_id` (and
//! transaction `owner`) fields from the stored session before dispatch; an
//! explicit caller value always wins.
//!
//! Streaming RPCs ([`GrpcClient::execute_query_stream`],
//! [`GrpcClient::stream_migration_progress`]) hand back the raw
//! `tonic::Streaming` values so callers control consumption and backpressure.
//!
//! Errors are mapped into the crate [`ClientError`]: connection establishment
//! failures become `ConnectionError`, while every non-OK `tonic::Status`
//! (including call-time transport failures) becomes a structured `GrpcError`
//! carrying a [`GrpcErrorCode`] and the server message.

use std::sync::RwLock;
use std::time::Duration;

use tonic::transport::{Channel, Endpoint};

use crate::config::ClientConfig;
use crate::utils::error::{ClientError, GrpcErrorCode, Result};

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
    max_message_size: Option<usize>,
}

impl GrpcClient {
    /// Create a client for `host:port` with default settings (lazy channel).
    pub fn new(host: &str, port: u16) -> Result<Self> {
        Self::with_config(ClientConfig::new().with_host(host).with_port(port))
    }

    /// Create a client from a [`ClientConfig`] (lazy channel). The configured
    /// timeout bounds both connection establishment and individual calls.
    ///
    /// [`ClientConfig`] defaults to the HTTP port; pass the gRPC port explicitly
    /// (the server default is `9669`).
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        let endpoint = endpoint_address(&config.host, config.port);
        let channel = build_endpoint(&endpoint, config.timeout_seconds)?.connect_lazy();
        Ok(Self::from_channel(channel, endpoint))
    }

    /// Create a client from a complete endpoint URI such as `http://host:9669`
    /// or `https://host:9669` (lazy channel). Scheme handling is delegated to
    /// tonic; `https` only passes through because this crate does not enable
    /// tonic's `tls` feature, so TLS negotiation must be added separately.
    pub fn from_endpoint(endpoint: impl Into<String>, timeout_seconds: u64) -> Result<Self> {
        let endpoint = endpoint.into();
        let channel = build_endpoint(&endpoint, timeout_seconds)?.connect_lazy();
        Ok(Self::from_channel(channel, endpoint))
    }

    /// Connect to `host:port` eagerly, failing if the server is unreachable.
    pub async fn connect(host: &str, port: u16) -> Result<Self> {
        let endpoint = endpoint_address(host, port);
        Self::connect_endpoint(endpoint, ClientConfig::default().timeout_seconds).await
    }

    /// Connect to a complete endpoint URI eagerly.
    pub async fn connect_endpoint(
        endpoint: impl Into<String>,
        timeout_seconds: u64,
    ) -> Result<Self> {
        let endpoint = endpoint.into();
        let channel = build_endpoint(&endpoint, timeout_seconds)?
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

    /// Configure the maximum decoded/encoded message size for subsequent calls.
    ///
    /// The server defaults to 10 MB while tonic clients default to 4 MB, so
    /// setting this to the server value avoids truncation errors on large
    /// query results.
    pub fn with_message_size_limit(mut self, bytes: usize) -> Self {
        self.max_message_size = Some(bytes);
        self
    }

    fn from_channel(channel: Channel, endpoint: String) -> Self {
        Self {
            inner: LinkrsServiceClient::new(channel),
            endpoint,
            session: RwLock::new(None),
            max_message_size: None,
        }
    }

    fn client(&self) -> LinkrsServiceClient<Channel> {
        let mut client = self.inner.clone();
        if let Some(bytes) = self.max_message_size {
            client = client
                .max_decoding_message_size(bytes)
                .max_encoding_message_size(bytes);
        }
        client
    }

    /// Endpoint address the client talks to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Session id issued by the last successful [`GrpcClient::login`].
    pub fn session_id(&self) -> Option<i64> {
        self.stored_session().and_then(|id| id.parse().ok())
    }

    /// Restore or replace the session used by subsequent protected calls.
    pub fn set_session_id(&self, session_id: i64) {
        self.store_session(session_id.to_string());
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

    fn fill_optional_session(&self, slot: &mut Option<String>) {
        if slot.as_deref().is_none_or(|value| value.is_empty()) {
            *slot = self.stored_session();
        }
    }

    fn fill_required_session(&self, slot: &mut String) -> Result<()> {
        if slot.is_empty() {
            *slot = self.require_session()?;
        }
        Ok(())
    }

    fn fill_optional_session_i64(&self, slot: &mut i64) -> Result<()> {
        if *slot == 0 {
            *slot = self
                .session_id()
                .ok_or_else(|| ClientError::auth("no active gRPC session; call login() first"))?;
        }
        Ok(())
    }

    fn fill_optional_owner(&self, slot: &mut Option<String>) {
        if slot.as_deref().is_none_or(|value| value.is_empty()) {
            *slot = self.stored_session();
        }
    }

    // ================================================================
    // Health
    // ================================================================

    /// Check server health, version, and uptime.
    pub async fn health_check(&self) -> Result<HealthCheckResponse> {
        let mut client = self.client();
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
        self.login_in_space(username, password, None).await
    }

    /// Authenticate against a specific space and remember the session id.
    pub async fn login_in_space(
        &self,
        username: &str,
        password: &str,
        space: Option<&str>,
    ) -> Result<LoginResponse> {
        let request = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            space: space.map(str::to_string),
        };
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
        client
            .auth_me(self.authed(tonic::Request::new(AuthMeRequest { session_id })))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Sessions
    // ================================================================

    /// Create a server-side session and remember its id on success.
    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        let mut client = self.client();
        let response = client
            .create_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)?;
        if response.success && !response.session_id.is_empty() {
            self.store_session(response.session_id.clone());
        }
        Ok(response)
    }

    /// Inspect a server-side session.
    pub async fn get_session(&self, mut request: GetSessionRequest) -> Result<GetSessionResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Close a server-side session.
    pub async fn close_session(
        &self,
        mut request: CloseSessionRequest,
    ) -> Result<CloseSessionResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .close_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List active server-side sessions.
    pub async fn list_sessions(
        &self,
        mut request: ListSessionsRequest,
    ) -> Result<ListSessionsResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .list_sessions(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Terminate another session as an administrator.
    pub async fn kill_session(
        &self,
        mut request: KillSessionRequest,
    ) -> Result<KillSessionResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .kill_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List variables of a session.
    pub async fn list_session_variables(
        &self,
        mut request: ListSessionVariablesRequest,
    ) -> Result<ListSessionVariablesResponse> {
        self.fill_optional_session_i64(&mut request.session_id)?;
        let mut client = self.client();
        client
            .list_session_variables(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Set a session variable.
    pub async fn set_session_variable(
        &self,
        mut request: SetSessionVariableRequest,
    ) -> Result<SetSessionVariableResponse> {
        self.fill_optional_session_i64(&mut request.session_id)?;
        let mut client = self.client();
        client
            .set_session_variable(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Delete a session variable.
    pub async fn delete_session_variable(
        &self,
        mut request: DeleteSessionVariableRequest,
    ) -> Result<DeleteSessionVariableResponse> {
        self.fill_optional_session_i64(&mut request.session_id)?;
        let mut client = self.client();
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
        mut request: ExecuteQueryRequest,
    ) -> Result<ExecuteQueryResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .execute_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Validate a query without executing it.
    pub async fn validate_query(
        &self,
        mut request: ValidateQueryRequest,
    ) -> Result<ValidateQueryResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .validate_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Return the query plan for a query.
    pub async fn explain_query(
        &self,
        mut request: ExplainQueryRequest,
    ) -> Result<ExplainQueryResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
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
        mut request: ExecuteQueryRequest,
    ) -> Result<tonic::Streaming<StreamResponse>> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .execute_query_stream(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List currently running queries.
    pub async fn list_queries(&self) -> Result<ListQueriesResponse> {
        let mut client = self.client();
        client
            .list_queries(self.authed(tonic::Request::new(ListQueriesRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Kill a running query.
    pub async fn kill_query(&self, mut request: KillQueryRequest) -> Result<KillQueryResponse> {
        self.fill_optional_session_i64(&mut request.session_id)?;
        let mut client = self.client();
        client
            .kill_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Open a cursor for paged query result retrieval.
    pub async fn open_cursor(&self, mut request: OpenCursorRequest) -> Result<OpenCursorResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .open_cursor(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch the next page of a cursor.
    pub async fn fetch_cursor(
        &self,
        mut request: FetchCursorRequest,
    ) -> Result<FetchCursorResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .fetch_cursor(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Close a cursor and release its server-side resources.
    pub async fn close_cursor(
        &self,
        mut request: CloseCursorRequest,
    ) -> Result<CloseCursorResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
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
        mut request: BeginTransactionRequest,
    ) -> Result<BeginTransactionResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .begin_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Commit a transaction.
    pub async fn commit_transaction(
        &self,
        mut request: CommitTransactionRequest,
    ) -> Result<CommitTransactionResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .commit_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back a transaction.
    pub async fn rollback_transaction(
        &self,
        mut request: RollbackTransactionRequest,
    ) -> Result<RollbackTransactionResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .rollback_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create a savepoint inside a transaction.
    pub async fn create_savepoint(
        &self,
        mut request: CreateSavepointRequest,
    ) -> Result<CreateSavepointResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .create_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back to a savepoint.
    pub async fn rollback_to_savepoint(
        &self,
        mut request: RollbackToSavepointRequest,
    ) -> Result<RollbackToSavepointResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .rollback_to_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Release a savepoint.
    pub async fn release_savepoint(
        &self,
        mut request: ReleaseSavepointRequest,
    ) -> Result<ReleaseSavepointResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .release_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List active transactions.
    pub async fn list_transactions(&self) -> Result<ListTransactionsResponse> {
        let mut client = self.client();
        client
            .list_transactions(self.authed(tonic::Request::new(ListTransactionsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Kill a transaction.
    pub async fn kill_transaction(
        &self,
        mut request: KillTransactionRequest,
    ) -> Result<KillTransactionResponse> {
        self.fill_optional_owner(&mut request.owner);
        let mut client = self.client();
        client
            .kill_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch transaction manager metrics.
    pub async fn transaction_metrics(&self) -> Result<GetTransactionMetricsResponse> {
        let mut client = self.client();
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
        let mut client = self.client();
        client
            .create_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch a space by name.
    pub async fn get_space(&self, request: GetSpaceRequest) -> Result<GetSpaceResponse> {
        let mut client = self.client();
        client
            .get_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop a space.
    pub async fn drop_space(&self, request: DropSpaceRequest) -> Result<DropSpaceResponse> {
        let mut client = self.client();
        client
            .drop_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List spaces.
    pub async fn list_spaces(&self) -> Result<ListSpacesResponse> {
        let mut client = self.client();
        client
            .list_spaces(self.authed(tonic::Request::new(ListSpacesRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create a tag.
    pub async fn create_tag(&self, request: CreateTagRequest) -> Result<CreateTagResponse> {
        let mut client = self.client();
        client
            .create_tag(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch a tag definition.
    pub async fn get_tag(&self, request: GetTagRequest) -> Result<GetTagResponse> {
        let mut client = self.client();
        client
            .get_tag(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List tags in a space.
    pub async fn list_tags(&self, request: ListTagsRequest) -> Result<ListTagsResponse> {
        let mut client = self.client();
        client
            .list_tags(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop a tag.
    pub async fn drop_tag(&self, request: DropTagRequest) -> Result<DropTagResponse> {
        let mut client = self.client();
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
        let mut client = self.client();
        client
            .create_edge_type(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch an edge type definition.
    pub async fn get_edge_type(&self, request: GetEdgeTypeRequest) -> Result<GetEdgeTypeResponse> {
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
        client
            .update_config(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Reset a configuration key to its default.
    pub async fn reset_config(&self, request: ResetConfigRequest) -> Result<ResetConfigResponse> {
        let mut client = self.client();
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
        let mut client = self.client();
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
        mut request: GetSessionStatisticsRequest,
    ) -> Result<GetSessionStatisticsResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .get_session_statistics(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch query statistics.
    pub async fn get_query_statistics(
        &self,
        mut request: GetQueryStatisticsRequest,
    ) -> Result<GetQueryStatisticsResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .get_query_statistics(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch database statistics.
    pub async fn get_database_statistics(&self) -> Result<GetDatabaseStatisticsResponse> {
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
        client
            .get_batch_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Cancel a batch job.
    pub async fn cancel_batch(&self, request: CancelBatchRequest) -> Result<CancelBatchResponse> {
        let mut client = self.client();
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
        let mut client = self.client();
        client
            .migrate_plan(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Execute a previously planned migration.
    pub async fn execute_migration(
        &self,
        mut request: MigrateExecuteRequest,
    ) -> Result<MigrateExecuteResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .migrate_execute(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back an executed migration.
    pub async fn rollback_migration(
        &self,
        mut request: MigrateRollbackRequest,
    ) -> Result<MigrateRollbackResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
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
        let mut client = self.client();
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
        let mut client = self.client();
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
    pub async fn import_data(&self, mut request: ImportDataRequest) -> Result<ImportDataResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
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
        let mut client = self.client();
        client
            .get_import_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Export query results in the requested format.
    pub async fn export_data(&self, mut request: ExportDataRequest) -> Result<ExportDataResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .export_data(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
    // ================================================================
    // Users
    // ================================================================

    /// List users.
    pub async fn list_users(&self, mut request: ListUsersRequest) -> Result<ListUsersResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .list_users(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create user.
    pub async fn create_user(&self, mut request: CreateUserRequest) -> Result<CreateUserResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .create_user(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop user.
    pub async fn drop_user(&self, mut request: DropUserRequest) -> Result<DropUserResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .drop_user(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Reset user password.
    pub async fn reset_user_password(
        &self,
        mut request: ResetUserPasswordRequest,
    ) -> Result<ResetUserPasswordResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .reset_user_password(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Enable user.
    pub async fn enable_user(&self, mut request: EnableUserRequest) -> Result<EnableUserResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .enable_user(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Disable user.
    pub async fn disable_user(
        &self,
        mut request: DisableUserRequest,
    ) -> Result<DisableUserResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .disable_user(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Grant user role.
    pub async fn grant_user_role(
        &self,
        mut request: GrantUserRoleRequest,
    ) -> Result<GrantUserRoleResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .grant_user_role(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Revoke user role.
    pub async fn revoke_user_role(
        &self,
        mut request: RevokeUserRoleRequest,
    ) -> Result<RevokeUserRoleResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .revoke_user_role(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Custom Functions
    // ================================================================

    /// Register function.
    pub async fn register_function(
        &self,
        request: RegisterFunctionRequest,
    ) -> Result<RegisterFunctionResponse> {
        let mut client = self.client();
        client
            .register_function(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Unregister function.
    pub async fn unregister_function(
        &self,
        request: UnregisterFunctionRequest,
    ) -> Result<UnregisterFunctionResponse> {
        let mut client = self.client();
        client
            .unregister_function(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List functions.
    pub async fn list_functions(&self) -> Result<ListFunctionsResponse> {
        let mut client = self.client();
        client
            .list_functions(self.authed(tonic::Request::new(ListFunctionsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get function info.
    pub async fn get_function_info(
        &self,
        request: GetFunctionInfoRequest,
    ) -> Result<GetFunctionInfoResponse> {
        let mut client = self.client();
        client
            .get_function_info(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Vector
    // ================================================================

    /// Create vector index.
    pub async fn create_vector_index(
        &self,
        request: CreateVectorIndexRequest,
    ) -> Result<CreateVectorIndexResponse> {
        let mut client = self.client();
        client
            .create_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get vector index.
    pub async fn get_vector_index(
        &self,
        request: GetVectorIndexRequest,
    ) -> Result<GetVectorIndexResponse> {
        let mut client = self.client();
        client
            .get_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List vector indexes.
    pub async fn list_vector_indexes(
        &self,
        request: ListVectorIndexesRequest,
    ) -> Result<ListVectorIndexesResponse> {
        let mut client = self.client();
        client
            .list_vector_indexes(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop vector index.
    pub async fn drop_vector_index(
        &self,
        request: DropVectorIndexRequest,
    ) -> Result<DropVectorIndexResponse> {
        let mut client = self.client();
        client
            .drop_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Search vector.
    pub async fn search_vector(
        &self,
        request: SearchVectorRequest,
    ) -> Result<SearchVectorResponse> {
        let mut client = self.client();
        client
            .search_vector(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Schema Versioning
    // ================================================================

    /// Get schema changes.
    pub async fn get_schema_changes(
        &self,
        request: SchemaChangesRequest,
    ) -> Result<SchemaChangesResponse> {
        let mut client = self.client();
        client
            .get_schema_changes(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Detect breaking changes.
    pub async fn detect_breaking_changes(
        &self,
        request: BreakingChangesRequest,
    ) -> Result<BreakingChangesResponse> {
        let mut client = self.client();
        client
            .detect_breaking_changes(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Migration Extensions
    // ================================================================

    /// Dry run migration.
    pub async fn dry_run_migration(
        &self,
        mut request: DryRunMigrationRequest,
    ) -> Result<DryRunMigrationResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .dry_run_migration(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get migration history.
    pub async fn get_migration_history(
        &self,
        mut request: GetMigrationHistoryRequest,
    ) -> Result<GetMigrationHistoryResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_migration_history(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get migration status.
    pub async fn get_migration_status(
        &self,
        mut request: GetMigrationStatusRequest,
    ) -> Result<GetMigrationStatusResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_migration_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Sync
    // ================================================================

    /// Get sync status.
    pub async fn get_sync_status(&self) -> Result<GetSyncStatusResponse> {
        let mut client = self.client();
        client
            .get_sync_status(self.authed(tonic::Request::new(GetSyncStatusRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Retry outbox.
    pub async fn retry_outbox(&self) -> Result<RetryOutboxResponse> {
        let mut client = self.client();
        client
            .retry_outbox(self.authed(tonic::Request::new(RetryOutboxRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get outbox diagnostics.
    pub async fn get_outbox_diagnostics(&self) -> Result<GetOutboxDiagnosticsResponse> {
        let mut client = self.client();
        client
            .get_outbox_diagnostics(
                self.authed(tonic::Request::new(GetOutboxDiagnosticsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List dead letters.
    pub async fn list_dead_letters(
        &self,
        request: ListDeadLettersRequest,
    ) -> Result<ListDeadLettersResponse> {
        let mut client = self.client();
        client
            .list_dead_letters(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Requeue dead letters.
    pub async fn requeue_dead_letters(
        &self,
        request: RequeueDeadLettersRequest,
    ) -> Result<RequeueDeadLettersResponse> {
        let mut client = self.client();
        client
            .requeue_dead_letters(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get retention status.
    pub async fn get_retention_status(&self) -> Result<GetRetentionStatusResponse> {
        let mut client = self.client();
        client
            .get_retention_status(self.authed(tonic::Request::new(GetRetentionStatusRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Run retention.
    pub async fn run_retention(
        &self,
        request: RunRetentionRequest,
    ) -> Result<RunRetentionResponse> {
        let mut client = self.client();
        client
            .run_retention(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Fulltext
    // ================================================================

    /// Rebuild fulltext index.
    pub async fn rebuild_fulltext_index(
        &self,
        mut request: RebuildFulltextIndexRequest,
    ) -> Result<RebuildFulltextIndexResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .rebuild_fulltext_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get fulltext rebuild status.
    pub async fn get_fulltext_rebuild_status(
        &self,
        mut request: GetFulltextRebuildStatusRequest,
    ) -> Result<GetFulltextRebuildStatusResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_fulltext_rebuild_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Clear fulltext index.
    pub async fn clear_fulltext_index(
        &self,
        mut request: ClearFulltextIndexRequest,
    ) -> Result<ClearFulltextIndexResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .clear_fulltext_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List inconsistent fulltext.
    pub async fn list_inconsistent_fulltext(
        &self,
        mut request: ListInconsistentFulltextRequest,
    ) -> Result<ListInconsistentFulltextResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .list_inconsistent_fulltext(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Search fulltext.
    pub async fn search_fulltext(
        &self,
        mut request: SearchFulltextRequest,
    ) -> Result<SearchFulltextResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .search_fulltext(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Query Extras
    // ================================================================

    /// Execute batch query.
    pub async fn execute_batch_query(
        &self,
        mut request: ExecuteBatchQueryRequest,
    ) -> Result<ExecuteBatchQueryResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .execute_batch_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Batch Extras
    // ================================================================

    /// Delete batch.
    pub async fn delete_batch(&self, request: DeleteBatchRequest) -> Result<DeleteBatchResponse> {
        let mut client = self.client();
        client
            .delete_batch(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Vector Extensions
    // ================================================================

    /// Scroll vector.
    pub async fn scroll_vector(
        &self,
        mut request: ScrollVectorRequest,
    ) -> Result<ScrollVectorResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .scroll_vector(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get vector count.
    pub async fn get_vector_count(
        &self,
        mut request: GetVectorCountRequest,
    ) -> Result<GetVectorCountResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_vector_count(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Rebuild vector index.
    pub async fn rebuild_vector_index(
        &self,
        mut request: RebuildVectorIndexRequest,
    ) -> Result<RebuildVectorIndexResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .rebuild_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get vector rebuild status.
    pub async fn get_vector_rebuild_status(
        &self,
        mut request: GetVectorRebuildStatusRequest,
    ) -> Result<GetVectorRebuildStatusResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_vector_rebuild_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Clear vector index.
    pub async fn clear_vector_index(
        &self,
        mut request: ClearVectorIndexRequest,
    ) -> Result<ClearVectorIndexResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .clear_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Statistics Extensions
    // ================================================================

    /// Get overview statistics.
    pub async fn get_overview_statistics(&self) -> Result<GetOverviewStatisticsResponse> {
        let mut client = self.client();
        client
            .get_overview_statistics(
                self.authed(tonic::Request::new(GetOverviewStatisticsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get search statistics.
    pub async fn get_search_statistics(&self) -> Result<GetSearchStatisticsResponse> {
        let mut client = self.client();
        client
            .get_search_statistics(self.authed(tonic::Request::new(GetSearchStatisticsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get migration statistics.
    pub async fn get_migration_statistics(&self) -> Result<GetMigrationStatisticsResponse> {
        let mut client = self.client();
        client
            .get_migration_statistics(
                self.authed(tonic::Request::new(GetMigrationStatisticsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get freeze statistics.
    pub async fn get_freeze_statistics(&self) -> Result<GetFreezeStatisticsResponse> {
        let mut client = self.client();
        client
            .get_freeze_statistics(self.authed(tonic::Request::new(GetFreezeStatisticsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Trigger freeze.
    pub async fn trigger_freeze(&self) -> Result<TriggerFreezeResponse> {
        let mut client = self.client();
        client
            .trigger_freeze(self.authed(tonic::Request::new(TriggerFreezeRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get query profile detail.
    pub async fn get_query_profile_detail(
        &self,
        request: GetQueryProfileDetailRequest,
    ) -> Result<GetQueryProfileDetailResponse> {
        let mut client = self.client();
        client
            .get_query_profile_detail(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Sync Degraded Ranges
    // ================================================================

    /// List degraded ranges.
    pub async fn list_degraded_ranges(
        &self,
        request: ListDegradedRangesRequest,
    ) -> Result<ListDegradedRangesResponse> {
        let mut client = self.client();
        client
            .list_degraded_ranges(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Clear degraded range.
    pub async fn clear_degraded_range(
        &self,
        request: ClearDegradedRangeRequest,
    ) -> Result<ClearDegradedRangeResponse> {
        let mut client = self.client();
        client
            .clear_degraded_range(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Transaction Extensions
    // ================================================================

    /// List savepoints.
    pub async fn list_savepoints(
        &self,
        mut request: ListSavepointsRequest,
    ) -> Result<ListSavepointsResponse> {
        self.fill_optional_owner(&mut request.owner);
        let mut client = self.client();
        client
            .list_savepoints(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Retry transaction outbox.
    pub async fn retry_transaction_outbox(
        &self,
        mut request: RetryTransactionOutboxRequest,
    ) -> Result<RetryTransactionOutboxResponse> {
        self.fill_optional_owner(&mut request.owner);
        let mut client = self.client();
        client
            .retry_transaction_outbox(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    // ================================================================
    // Vector Payload
    // ================================================================

    /// Get vector point.
    pub async fn get_vector_point(
        &self,
        request: GetVectorPointRequest,
    ) -> Result<GetVectorPointResponse> {
        let mut client = self.client();
        client
            .get_vector_point(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Set vector payload.
    pub async fn set_vector_payload(
        &self,
        request: SetVectorPayloadRequest,
    ) -> Result<SetVectorPayloadResponse> {
        let mut client = self.client();
        client
            .set_vector_payload(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Set vector payload fields.
    pub async fn set_vector_payload_fields(
        &self,
        request: SetVectorPayloadFieldsRequest,
    ) -> Result<SetVectorPayloadFieldsResponse> {
        let mut client = self.client();
        client
            .set_vector_payload_fields(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Delete vector payload.
    pub async fn delete_vector_payload(
        &self,
        request: DeleteVectorPayloadRequest,
    ) -> Result<DeleteVectorPayloadResponse> {
        let mut client = self.client();
        client
            .delete_vector_payload(self.authed(tonic::Request::new(request)))
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
    ClientError::grpc_status(grpc_code(status.code()), status.message())
}

fn grpc_code(code: tonic::Code) -> GrpcErrorCode {
    match code {
        tonic::Code::Ok => GrpcErrorCode::Other,
        tonic::Code::Cancelled => GrpcErrorCode::Cancelled,
        tonic::Code::Unknown => GrpcErrorCode::Unknown,
        tonic::Code::InvalidArgument => GrpcErrorCode::InvalidArgument,
        tonic::Code::DeadlineExceeded => GrpcErrorCode::DeadlineExceeded,
        tonic::Code::NotFound => GrpcErrorCode::NotFound,
        tonic::Code::AlreadyExists => GrpcErrorCode::AlreadyExists,
        tonic::Code::PermissionDenied => GrpcErrorCode::PermissionDenied,
        tonic::Code::ResourceExhausted => GrpcErrorCode::ResourceExhausted,
        tonic::Code::FailedPrecondition => GrpcErrorCode::FailedPrecondition,
        tonic::Code::Aborted => GrpcErrorCode::Aborted,
        tonic::Code::OutOfRange => GrpcErrorCode::OutOfRange,
        tonic::Code::Unimplemented => GrpcErrorCode::Unimplemented,
        tonic::Code::Internal => GrpcErrorCode::Internal,
        tonic::Code::Unavailable => GrpcErrorCode::Unavailable,
        tonic::Code::DataLoss => GrpcErrorCode::DataLoss,
        tonic::Code::Unauthenticated => GrpcErrorCode::Unauthenticated,
    }
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
            ClientError::GrpcError { code, message } => {
                assert_eq!(code, GrpcErrorCode::Unauthenticated);
                assert!(message.contains("bad credentials"), "message: {}", message);
            }
            other => panic!("expected grpc error, got {:?}", other),
        }
    }

    #[test]
    fn grpc_codes_map_to_structured_values() {
        assert_eq!(
            grpc_code(tonic::Code::PermissionDenied),
            GrpcErrorCode::PermissionDenied
        );
        assert_eq!(grpc_code(tonic::Code::NotFound), GrpcErrorCode::NotFound);
        assert_eq!(
            grpc_code(tonic::Code::Unavailable),
            GrpcErrorCode::Unavailable
        );
        assert_eq!(grpc_code(tonic::Code::Unknown), GrpcErrorCode::Unknown);
    }

    #[tokio::test]
    async fn optional_session_is_filled_only_when_missing() {
        let client = test_client();
        client.set_session_id(42);

        let mut slot = None;
        client.fill_optional_session(&mut slot);
        assert_eq!(slot.as_deref(), Some("42"));

        let mut explicit = Some("other".to_string());
        client.fill_optional_session(&mut explicit);
        assert_eq!(explicit.as_deref(), Some("other"));
    }

    #[tokio::test]
    async fn required_session_fails_without_login() {
        let client = test_client();
        let mut slot = String::new();
        let error = client
            .fill_required_session(&mut slot)
            .expect_err("expected auth error");
        assert!(matches!(error, ClientError::AuthError(_)));

        client.set_session_id(42);
        let mut slot = String::new();
        client
            .fill_required_session(&mut slot)
            .expect("session should be filled");
        assert_eq!(slot, "42");
    }

    #[tokio::test]
    async fn optional_int_session_and_owner_are_filled() {
        let client = test_client();
        client.set_session_id(42);

        let mut slot = 0;
        client
            .fill_optional_session_i64(&mut slot)
            .expect("session should be filled");
        assert_eq!(slot, 42);

        let mut owner = None;
        client.fill_optional_owner(&mut owner);
        assert_eq!(owner.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn endpoint_and_message_limit_configuration_is_applied() {
        let client = GrpcClient::from_endpoint("https://127.0.0.1:9669", 0)
            .expect("client should build")
            .with_message_size_limit(10 * 1024 * 1024);
        assert_eq!(client.endpoint(), "https://127.0.0.1:9669");
        assert_eq!(client.max_message_size, Some(10 * 1024 * 1024));
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
