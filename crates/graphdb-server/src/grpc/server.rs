//! gRPC Server Implementation
//!
//! Provides a gRPC-based interface to GraphDB services.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio_stream::StreamExt;
use tonic::{transport::Server, Request, Response, Status};

use crate::config::Config;
use crate::http::AppState;

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSnapshotOps,
    StorageSyncContextOps,
};
use graphdb_transaction::{
    DurabilityLevel, IsolationLevel, TransactionError, TransactionErrorKind, TransactionId,
    TransactionOptions,
};

// Import generated proto types
use super::proto::graph_db_service_server::{
    GraphDbService as GraphDBServiceTrait, GraphDbServiceServer,
};
use super::proto::*;

// Type alias for the streaming response
type ExecuteQueryStreamStream = std::pin::Pin<
    Box<dyn tokio_stream::Stream<Item = Result<StreamResponse, Status>> + Send + 'static>,
>;

type StreamMigrationProgressStream = std::pin::Pin<
    Box<dyn tokio_stream::Stream<Item = Result<MigrationProgressEvent, Status>> + Send + 'static>,
>;

/// GraphDB gRPC service implementation
pub struct GraphDBService<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + 'static,
> {
    app_state: AppState<S>,
    config: Config,
    start_time: Instant,
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > GraphDBService<S>
{
    /// Create a new gRPC service instance
    pub fn new(app_state: AppState<S>, config: Config) -> Self {
        Self {
            app_state,
            config,
            start_time: Instant::now(),
        }
    }

    /// Get the current configuration
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get application state
    pub fn app_state(&self) -> &AppState<S> {
        &self.app_state
    }
}

#[tonic::async_trait]
impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > GraphDBServiceTrait for GraphDBService<S>
{
    type ExecuteQueryStreamStream = ExecuteQueryStreamStream;
    type StreamMigrationProgressStream = StreamMigrationProgressStream;

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let uptime = self.start_time.elapsed().as_secs();

        Ok(Response::new(HealthCheckResponse {
            healthy: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: uptime as i64,
        }))
    }

    async fn login(
        &self,
        request: Request<LoginRequest>,
    ) -> Result<Response<LoginResponse>, Status> {
        let req = request.into_inner();
        if req.username.is_empty() || req.password.is_empty() {
            return Err(Status::unauthenticated(
                "username and password must not be empty",
            ));
        }
        let graph_service = self.app_state.server.get_graph_service();
        let session = graph_service
            .authenticate(&req.username, &req.password)
            .await
            .map_err(Status::unauthenticated)?;
        if let Some(space) = req.space.filter(|s| !s.is_empty()) {
            attach_session_space(&self.app_state, &session, &space)?;
        }
        Ok(Response::new(LoginResponse {
            success: true,
            session_id: session.id().to_string(),
            error: String::new(),
        }))
    }

    async fn logout(
        &self,
        request: Request<LogoutRequest>,
    ) -> Result<Response<LogoutResponse>, Status> {
        let session_id = parse_session_id(&request.into_inner().session_id)?;
        self.app_state
            .server
            .get_session_manager()
            .remove_session(session_id)
            .await;
        Ok(Response::new(LogoutResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let req = request.into_inner();
        if req.username.is_empty() {
            return Err(Status::invalid_argument("username must not be empty"));
        }
        let session_manager = self.app_state.server.get_session_manager();
        let session = if req.password.is_empty() {
            session_manager
                .create_session(req.username.clone(), "127.0.0.1".to_string())
                .await
                .map_err(|e| Status::internal(format!("failed to create session: {e}")))?
        } else {
            let graph_service = self.app_state.server.get_graph_service();
            graph_service
                .authenticate(&req.username, &req.password)
                .await
                .map_err(Status::unauthenticated)?
        };
        if let Some(space) = req.space.filter(|s| !s.is_empty()) {
            attach_session_space(&self.app_state, &session, &space)?;
        }
        let space_id = session.space().map(|s| s.id as i32).unwrap_or(0);
        Ok(Response::new(CreateSessionResponse {
            success: true,
            session_id: session.id().to_string(),
            space_id,
            error: String::new(),
        }))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let raw_id = request.into_inner().session_id;
        let session_id = parse_session_id(&raw_id)?;
        let session_manager = self.app_state.server.get_session_manager();
        match session_manager.get_session_info(session_id).await {
            None => Ok(Response::new(GetSessionResponse {
                exists: false,
                session_id: raw_id,
                username: String::new(),
                space_id: 0,
                created_at: 0,
                last_accessed: 0,
            })),
            Some(info) => {
                let space_id = session_manager
                    .find_session(session_id)
                    .and_then(|s| s.space())
                    .map(|s| s.id as i32)
                    .unwrap_or(0);
                Ok(Response::new(GetSessionResponse {
                    exists: true,
                    session_id: raw_id,
                    username: info.user_name,
                    space_id,
                    created_at: system_time_secs(&info.create_time),
                    last_accessed: system_time_secs(&info.last_access_time),
                }))
            }
        }
    }

    async fn close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        let session_id = parse_session_id(&request.into_inner().session_id)?;
        self.app_state
            .server
            .get_session_manager()
            .remove_session(session_id)
            .await;
        Ok(Response::new(CloseSessionResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn execute_query(
        &self,
        request: Request<ExecuteQueryRequest>,
    ) -> Result<Response<ExecuteQueryResponse>, Status> {
        let req = request.into_inner();
        if req.query.trim().is_empty() {
            return Err(Status::invalid_argument("query must not be empty"));
        }
        let session_id = match req.session_id {
            Some(raw) => parse_session_id(&raw)?,
            None => return Err(Status::unauthenticated("session_id is required")),
        };
        let parameters = req.parameters.map(|p| {
            p.params
                .into_iter()
                .map(|(k, v)| (k, proto_value_to_core(v)))
                .collect::<HashMap<String, graphdb_core::Value>>()
        });
        let graph_service = self.app_state.server.get_graph_service();
        match graph_service
            .execute_with_params(session_id, &req.query, parameters, None)
            .await
        {
            Ok(result) => {
                // Configuration intents resolve against the live store like
                // the HTTP surface; resolution failures are query errors.
                let store = self.app_state.server.config_store();
                let config_path = self.app_state.server.get_config_path();
                match crate::http::handlers::config::resolve_query_config_intent(
                    result,
                    &store,
                    config_path.as_deref(),
                ) {
                    Ok(resolved) => {
                        let (query_result, metadata) = query_result_to_proto(&resolved);
                        Ok(Response::new(ExecuteQueryResponse {
                            success: true,
                            result: query_result,
                            error: String::new(),
                            metadata,
                        }))
                    }
                    Err(e) => Ok(Response::new(ExecuteQueryResponse {
                        success: false,
                        result: None,
                        error: e,
                        metadata: None,
                    })),
                }
            }
            Err(e) if e.contains("Invalid session ID") => Err(Status::unauthenticated(e)),
            Err(e) => Ok(Response::new(ExecuteQueryResponse {
                success: false,
                result: None,
                error: e,
                metadata: None,
            })),
        }
    }

    async fn validate_query(
        &self,
        request: Request<ValidateQueryRequest>,
    ) -> Result<Response<ValidateQueryResponse>, Status> {
        let query = request.into_inner().query;
        match crate::http::handlers::query::validate_gql(&query) {
            Ok(parameter_names) => Ok(Response::new(ValidateQueryResponse {
                valid: true,
                error: String::new(),
                parameter_names,
            })),
            Err(e) => Ok(Response::new(ValidateQueryResponse {
                valid: false,
                error: e,
                parameter_names: vec![],
            })),
        }
    }

    async fn execute_query_stream(
        &self,
        request: Request<ExecuteQueryRequest>,
    ) -> Result<Response<Self::ExecuteQueryStreamStream>, Status> {
        let inner = request.into_inner();
        let session_id: i64 = inner.session_id.unwrap_or_default().parse().unwrap_or(0);
        let query = inner.query;

        let graph_service = self.app_state.server.get_graph_service();

        let stream_result = graph_service
            .execute_stream(session_id, &query)
            .await
            .map_err(Status::internal)?;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamResponse, Status>>(16);

        // If column names are already known (from fallback), send schema immediately.
        let schema_sent: std::sync::Arc<std::sync::atomic::AtomicBool> =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        if let Some(cols) = stream_result.column_names() {
            let msg = super::proto::StreamResponse {
                payload: Some(super::proto::stream_response::Payload::Schema(
                    super::proto::SchemaMessage { column_names: cols },
                )),
            };
            if tx.blocking_send(Ok(msg)).is_err() {
                return Ok(Response::new(
                    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
                        as Self::ExecuteQueryStreamStream,
                ));
            }
            schema_sent.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        let schema_sent_clone = schema_sent.clone();

        tokio::spawn(async move {
            let tx_pull = tx.clone();

            let pull_handle = tokio::task::spawn_blocking(move || {
                loop {
                    match stream_result.next_chunk() {
                        Ok(Some(chunk)) => {
                            // Send schema from first chunk if not already sent.
                            if !schema_sent_clone.load(std::sync::atomic::Ordering::Relaxed) {
                                let cols = chunk.col_names();
                                let schema_msg = super::proto::StreamResponse {
                                    payload: Some(super::proto::stream_response::Payload::Schema(
                                        super::proto::SchemaMessage { column_names: cols },
                                    )),
                                };
                                if tx_pull.blocking_send(Ok(schema_msg)).is_err() {
                                    stream_result.cancel();
                                    return;
                                }
                                schema_sent_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                            }

                            let column_names = chunk.col_names();

                            let proto_rows: Vec<super::proto::Row> = chunk
                                .rows
                                .into_iter()
                                .map(|row| {
                                    let values: Vec<super::proto::Value> =
                                        row.into_iter().map(value_to_proto_value).collect();
                                    super::proto::Row { values }
                                })
                                .collect();

                            let proto_chunk = super::proto::StreamResponse {
                                payload: Some(super::proto::stream_response::Payload::Data(
                                    super::proto::QueryResultChunk {
                                        rows: proto_rows,
                                        is_last: false,
                                        column_names,
                                    },
                                )),
                            };

                            if tx_pull.blocking_send(Ok(proto_chunk)).is_err() {
                                // Client disconnected — cancel the query.
                                stream_result.cancel();
                                return;
                            }
                        }
                        Ok(None) => {
                            // Send final empty data message with is_last=true
                            let _ = tx_pull.blocking_send(Ok(super::proto::StreamResponse {
                                payload: Some(super::proto::stream_response::Payload::Data(
                                    super::proto::QueryResultChunk {
                                        rows: vec![],
                                        is_last: true,
                                        column_names: vec![],
                                    },
                                )),
                            }));
                            return;
                        }
                        Err(e) => {
                            let _ = tx_pull.blocking_send(Err(Status::internal(e.to_string())));
                            return;
                        }
                    }
                }
            });

            let _ = pull_handle.await;
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(stream) as Self::ExecuteQueryStreamStream
        ))
    }

    async fn begin_transaction(
        &self,
        request: Request<BeginTransactionRequest>,
    ) -> Result<Response<BeginTransactionResponse>, Status> {
        let request = request.into_inner();
        let proto_options = request.options.unwrap_or_default();
        let isolation_level = match proto_options.isolation_level {
            0 => IsolationLevel::RepeatableRead,
            1 => IsolationLevel::ReadCommitted,
            2 => IsolationLevel::Serializable,
            value => {
                return Err(Status::invalid_argument(format!(
                    "Unsupported isolation level: {}",
                    value
                )))
            }
        };
        let options = TransactionOptions {
            timeout: (proto_options.timeout_ms > 0).then_some(std::time::Duration::from_millis(
                proto_options.timeout_ms as u64,
            )),
            read_only: proto_options.read_only,
            long_read: false,
            durability: DurabilityLevel::Sync,
            isolation_level,
            query_timeout: None,
            statement_timeout: None,
            idle_timeout: None,
        };
        let manager = self.app_state.server.get_txn_manager();
        let txn_id = match request.session_id {
            Some(owner) => manager.begin_transaction_with_owner(options, owner),
            None => manager.begin_transaction(options),
        }
        .map_err(transaction_status)?;

        Ok(Response::new(BeginTransactionResponse {
            success: true,
            transaction_id: txn_id.as_u64().to_string(),
            error: String::new(),
        }))
    }

    async fn commit_transaction(
        &self,
        request: Request<CommitTransactionRequest>,
    ) -> Result<Response<CommitTransactionResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        manager
            .commit_transaction(txn_id)
            .map_err(transaction_status)?;
        Ok(Response::new(CommitTransactionResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn rollback_transaction(
        &self,
        request: Request<RollbackTransactionRequest>,
    ) -> Result<Response<RollbackTransactionResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        manager
            .abort_transaction(txn_id)
            .map_err(transaction_status)?;
        Ok(Response::new(RollbackTransactionResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn create_savepoint(
        &self,
        request: Request<CreateSavepointRequest>,
    ) -> Result<Response<CreateSavepointResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        let savepoint_id = manager
            .create_savepoint(txn_id, request.name)
            .map_err(transaction_status)?;
        Ok(Response::new(CreateSavepointResponse {
            success: true,
            savepoint_id,
            error: String::new(),
        }))
    }

    async fn rollback_to_savepoint(
        &self,
        request: Request<RollbackToSavepointRequest>,
    ) -> Result<Response<RollbackToSavepointResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        manager
            .rollback_to_savepoint(txn_id, request.savepoint_id, &*storage_guard)
            .map_err(transaction_status)?;
        Ok(Response::new(RollbackToSavepointResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn release_savepoint(
        &self,
        request: Request<ReleaseSavepointRequest>,
    ) -> Result<Response<ReleaseSavepointResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        manager
            .get_context(txn_id)
            .map_err(transaction_status)?
            .release_savepoint(request.savepoint_id)
            .map_err(transaction_status)?;
        Ok(Response::new(ReleaseSavepointResponse {
            success: true,
            error: String::new(),
        }))
    }

    // Schema Management - Space
    async fn create_space(
        &self,
        request: Request<CreateSpaceRequest>,
    ) -> Result<Response<CreateSpaceResponse>, Status> {
        let req = request.into_inner();
        if req.name.is_empty() {
            return Err(Status::invalid_argument("space name must not be empty"));
        }
        let mut info = graphdb_core::types::SpaceInfo::new(req.name.clone());
        if let Some(options) = req.options {
            if options.partition_num > 0 {
                info.partition_num = options.partition_num;
            }
            if options.replica_num > 0 {
                info.replica_factor = options.replica_num;
            }
        }
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let created = storage_guard
            .create_space(&mut info)
            .map_err(|e| Status::internal(format!("failed to create space: {e}")))?;
        if !created {
            return Err(Status::already_exists(format!(
                "space '{}' already exists",
                req.name
            )));
        }
        let space_id = storage_guard
            .get_space_id(&req.name)
            .map_err(|e| Status::internal(format!("failed to resolve new space: {e}")))?;
        Ok(Response::new(CreateSpaceResponse {
            success: true,
            space_id: space_id as i32,
            error: String::new(),
        }))
    }

    async fn get_space(
        &self,
        request: Request<GetSpaceRequest>,
    ) -> Result<Response<GetSpaceResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let space = storage_guard
            .get_space(&req.name)
            .map_err(|e| Status::internal(format!("failed to get space: {e}")))?;
        match space {
            Some(info) => Ok(Response::new(GetSpaceResponse {
                exists: true,
                space: Some(core_space_to_proto(&info)),
                error: String::new(),
            })),
            None => Ok(Response::new(GetSpaceResponse {
                exists: false,
                space: None,
                error: String::new(),
            })),
        }
    }

    async fn drop_space(
        &self,
        request: Request<DropSpaceRequest>,
    ) -> Result<Response<DropSpaceResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let dropped = storage_guard
            .drop_space(&req.name)
            .map_err(|e| Status::internal(format!("failed to drop space: {e}")))?;
        if dropped || req.if_exists {
            Ok(Response::new(DropSpaceResponse {
                success: true,
                error: String::new(),
            }))
        } else {
            Err(Status::not_found(format!(
                "space '{}' does not exist",
                req.name
            )))
        }
    }

    async fn list_spaces(
        &self,
        _request: Request<ListSpacesRequest>,
    ) -> Result<Response<ListSpacesResponse>, Status> {
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let spaces = storage_guard
            .list_spaces()
            .map_err(|e| Status::internal(format!("failed to list spaces: {e}")))?;
        Ok(Response::new(ListSpacesResponse {
            spaces: spaces.iter().map(core_space_to_proto).collect(),
            error: String::new(),
        }))
    }

    // Schema Management - Tag
    async fn create_tag(
        &self,
        request: Request<CreateTagRequest>,
    ) -> Result<Response<CreateTagResponse>, Status> {
        let req = request.into_inner();
        if req.space_name.is_empty() || req.tag_name.is_empty() {
            return Err(Status::invalid_argument(
                "space_name and tag_name must not be empty",
            ));
        }
        let properties = req
            .properties
            .into_iter()
            .map(proto_property_to_core)
            .collect::<Vec<_>>();
        let mut tag_info =
            graphdb_core::types::TagInfo::new(req.tag_name.clone()).with_properties(properties);
        if let Some(options) = req.options {
            let ttl = (options.ttl_seconds > 0).then_some(options.ttl_seconds);
            let col = (!options.ttl_column.is_empty()).then(|| options.ttl_column.clone());
            tag_info = tag_info.with_ttl(ttl, col);
        }
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let tag_id = storage_guard
            .create_tag(&req.space_name, &tag_info)
            .map_err(|e| Status::internal(format!("failed to create tag: {e}")))?;
        Ok(Response::new(CreateTagResponse {
            success: true,
            tag_id: tag_id as i32,
            error: String::new(),
        }))
    }

    async fn get_tag(
        &self,
        request: Request<GetTagRequest>,
    ) -> Result<Response<GetTagResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let tag = storage_guard
            .get_tag(&req.space_name, &req.tag_name)
            .map_err(|e| Status::internal(format!("failed to get tag: {e}")))?;
        match tag {
            Some(info) => Ok(Response::new(GetTagResponse {
                exists: true,
                tag: Some(core_tag_to_proto(&info)),
                error: String::new(),
            })),
            None => Ok(Response::new(GetTagResponse {
                exists: false,
                tag: None,
                error: String::new(),
            })),
        }
    }

    async fn list_tags(
        &self,
        request: Request<ListTagsRequest>,
    ) -> Result<Response<ListTagsResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let tags = storage_guard
            .list_tags(&req.space_name)
            .map_err(|e| Status::internal(format!("failed to list tags: {e}")))?;
        Ok(Response::new(ListTagsResponse {
            tags: tags.iter().map(core_tag_to_proto).collect(),
            error: String::new(),
        }))
    }

    async fn drop_tag(
        &self,
        request: Request<DropTagRequest>,
    ) -> Result<Response<DropTagResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let dropped = storage_guard
            .drop_tag(&req.space_name, &req.tag_name)
            .map_err(|e| Status::internal(format!("failed to drop tag: {e}")))?;
        if dropped || req.if_exists {
            Ok(Response::new(DropTagResponse {
                success: true,
                error: String::new(),
            }))
        } else {
            Err(Status::not_found(format!(
                "tag '{}' does not exist",
                req.tag_name
            )))
        }
    }

    // Schema Management - Edge Type
    async fn create_edge_type(
        &self,
        request: Request<CreateEdgeTypeRequest>,
    ) -> Result<Response<CreateEdgeTypeResponse>, Status> {
        let req = request.into_inner();
        if req.space_name.is_empty() || req.edge_type_name.is_empty() {
            return Err(Status::invalid_argument(
                "space_name and edge_type_name must not be empty",
            ));
        }
        let properties = req
            .properties
            .into_iter()
            .map(proto_property_to_core)
            .collect::<Vec<_>>();
        let edge_info = graphdb_core::types::EdgeTypeInfo::new(req.edge_type_name.clone())
            .with_properties(properties);
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let edge_type_id = storage_guard
            .create_edge_type(&req.space_name, &edge_info)
            .map_err(|e| Status::internal(format!("Failed to create edge type: {e}")))?;
        Ok(Response::new(CreateEdgeTypeResponse {
            success: true,
            edge_type_id: edge_type_id as i32,
            error: String::new(),
        }))
    }

    async fn get_edge_type(
        &self,
        request: Request<GetEdgeTypeRequest>,
    ) -> Result<Response<GetEdgeTypeResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let edge = storage_guard
            .get_edge_type(&req.space_name, &req.edge_type_name)
            .map_err(|e| Status::internal(format!("Failed to get edge type: {e}")))?;
        match edge {
            Some(info) => Ok(Response::new(GetEdgeTypeResponse {
                exists: true,
                edge_type: Some(core_edge_info_to_proto(&info)),
                error: String::new(),
            })),
            None => Ok(Response::new(GetEdgeTypeResponse {
                exists: false,
                edge_type: None,
                error: String::new(),
            })),
        }
    }

    async fn list_edge_types(
        &self,
        request: Request<ListEdgeTypesRequest>,
    ) -> Result<Response<ListEdgeTypesResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let edges = storage_guard
            .list_edge_types(&req.space_name)
            .map_err(|e| Status::internal(format!("Failed to list edge types: {e}")))?;
        Ok(Response::new(ListEdgeTypesResponse {
            edge_types: edges.iter().map(core_edge_info_to_proto).collect(),
            error: String::new(),
        }))
    }

    async fn drop_edge_type(
        &self,
        request: Request<DropEdgeTypeRequest>,
    ) -> Result<Response<DropEdgeTypeResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let mut storage_guard = storage.write();
        let dropped = storage_guard
            .drop_edge_type(&req.space_name, &req.edge_type_name)
            .map_err(|e| Status::internal(format!("Failed to drop edge type: {e}")))?;
        if dropped || req.if_exists {
            Ok(Response::new(DropEdgeTypeResponse {
                success: true,
                error: String::new(),
            }))
        } else {
            Ok(Response::new(DropEdgeTypeResponse {
                success: false,
                error: format!("Edge type '{}' does not exist", req.edge_type_name),
            }))
        }
    }

    // Batch Operations
    //
    // Batch tasks buffer items per space and flush continue-on-error:
    // inserts go through the core batch operation, updates and deletes run
    // against storage writer calls with per-item error capture.
    async fn create_batch(
        &self,
        request: Request<CreateBatchRequest>,
    ) -> Result<Response<CreateBatchResponse>, Status> {
        let req = request.into_inner();
        if req.space_name.is_empty() {
            return Err(Status::invalid_argument("space_name must not be empty"));
        }
        let storage = self.app_state.server.get_storage();
        let space_id = storage
            .read()
            .get_space_id(&req.space_name)
            .map_err(|_| Status::not_found(format!("space '{}' not found", req.space_name)))?;
        let batch_manager = self.app_state.server.get_batch_manager();
        let task = batch_manager
            .create_task(space_id, crate::batch::BatchType::Mixed, 1000)
            .map_err(|e| Status::internal(format!("failed to create batch task: {e}")))?;
        Ok(Response::new(CreateBatchResponse {
            success: true,
            batch_id: task.id,
            error: String::new(),
        }))
    }

    async fn add_batch_items(
        &self,
        request: Request<AddBatchItemsRequest>,
    ) -> Result<Response<AddBatchItemsResponse>, Status> {
        let req = request.into_inner();
        if req.items.is_empty() {
            return Err(Status::invalid_argument("batch items must not be empty"));
        }
        let mut wire_items = Vec::with_capacity(req.items.len());
        for item in req.items {
            wire_items.push(proto_batch_item_to_wire(item)?);
        }
        let batch_manager = self.app_state.server.get_batch_manager();
        let accepted = batch_manager
            .add_items(&req.batch_id, wire_items)
            .map_err(|e| {
                let message = e.to_string();
                if message.contains("does not exist") {
                    Status::not_found(message)
                } else {
                    Status::failed_precondition(message)
                }
            })?;
        Ok(Response::new(AddBatchItemsResponse {
            success: true,
            items_added: accepted as i32,
            error: String::new(),
        }))
    }

    async fn execute_batch(
        &self,
        request: Request<ExecuteBatchRequest>,
    ) -> Result<Response<ExecuteBatchResponse>, Status> {
        let req = request.into_inner();
        let batch_manager = self.app_state.server.get_batch_manager();
        let task = batch_manager
            .get_task(&req.batch_id)
            .ok_or_else(|| Status::not_found(format!("batch task '{}' not found", req.batch_id)))?;
        let space_name = {
            let storage = self.app_state.server.get_storage();
            let storage_guard = storage.read();
            storage_guard
                .get_space_by_id(task.space_id)
                .map_err(|e| Status::internal(format!("failed to resolve batch space: {e}")))?
                .map(|info| info.space_name)
                .ok_or_else(|| Status::not_found(format!("space id {} not found", task.space_id)))?
        };
        let result = batch_manager
            .execute_task(&req.batch_id, &space_name)
            .await
            .map_err(|e| Status::internal(format!("failed to execute batch task: {e}")))?;
        let success = result.errors.is_empty();
        let mut results: Vec<BatchResult> = result
            .errors
            .iter()
            .map(|e| BatchResult {
                success: false,
                error: e.error.clone(),
            })
            .collect();
        if success {
            results.push(BatchResult {
                success: true,
                error: String::new(),
            });
        }
        Ok(Response::new(ExecuteBatchResponse {
            success,
            results,
            error: result
                .errors
                .iter()
                .map(|e| e.error.clone())
                .collect::<Vec<_>>()
                .join("; "),
            vertices_inserted: result.vertices_inserted as i64,
            edges_inserted: result.edges_inserted as i64,
            vertices_updated: result.vertices_updated as i64,
            edges_updated: result.edges_updated as i64,
            vertices_deleted: result.vertices_deleted as i64,
            edges_deleted: result.edges_deleted as i64,
        }))
    }

    async fn get_batch_status(
        &self,
        request: Request<GetBatchStatusRequest>,
    ) -> Result<Response<GetBatchStatusResponse>, Status> {
        let req = request.into_inner();
        let batch_manager = self.app_state.server.get_batch_manager();
        let task = batch_manager
            .get_task(&req.batch_id)
            .ok_or_else(|| Status::not_found(format!("batch task '{}' not found", req.batch_id)))?;
        Ok(Response::new(GetBatchStatusResponse {
            status: batch_status_name(&task.status),
            total_items: task.progress.total as i32,
            processed_items: task.progress.processed as i32,
            failed_items: task.progress.failed as i32,
            error: String::new(),
        }))
    }

    async fn cancel_batch(
        &self,
        request: Request<CancelBatchRequest>,
    ) -> Result<Response<CancelBatchResponse>, Status> {
        let req = request.into_inner();
        let batch_manager = self.app_state.server.get_batch_manager();
        match batch_manager.cancel_task(&req.batch_id) {
            Ok(()) => Ok(Response::new(CancelBatchResponse {
                success: true,
                error: String::new(),
            })),
            Err(e) => {
                let message = e.to_string();
                if message.contains("does not exist") {
                    Err(Status::not_found(message))
                } else {
                    Err(Status::failed_precondition(message))
                }
            }
        }
    }

    // Statistics
    async fn get_session_statistics(
        &self,
        request: Request<GetSessionStatisticsRequest>,
    ) -> Result<Response<GetSessionStatisticsResponse>, Status> {
        let req = request.into_inner();
        let session_manager = self.app_state.server.get_session_manager();
        if let Some(raw) = req.session_id.filter(|s| !s.is_empty()) {
            let session_id = parse_session_id(&raw)?;
            if session_manager.find_session(session_id).is_none() {
                return Err(Status::not_found(format!("session '{raw}' not found")));
            }
        }
        let sessions = session_manager.list_sessions().await;
        let mut by_user: HashMap<String, i64> = HashMap::new();
        for session in &sessions {
            *by_user.entry(session.user_name.clone()).or_insert(0) += 1;
        }
        let failed = self
            .app_state
            .server
            .get_stats_manager()
            .get_value(graphdb_metrics::MetricType::NumAuthFailedSessions)
            .unwrap_or(0) as i64;
        let total = session_manager.total_sessions_created() as i64;
        Ok(Response::new(GetSessionStatisticsResponse {
            active_sessions: sessions.len() as i64,
            total_sessions: total,
            failed_sessions: failed,
            session_by_user: by_user,
        }))
    }

    async fn get_query_statistics(
        &self,
        request: Request<GetQueryStatisticsRequest>,
    ) -> Result<Response<GetQueryStatisticsResponse>, Status> {
        let req = request.into_inner();
        // from/to_timestamp are epoch millis; the profile buffer only keeps
        // monotonic start times, so wall time is derived as now - elapsed.
        let from_ms = req.from_timestamp.unwrap_or(0);
        let to_ms = req.to_timestamp.unwrap_or(i64::MAX);
        if from_ms < 0 || to_ms < 0 || from_ms > to_ms {
            return Err(Status::invalid_argument(
                "from_timestamp/to_timestamp must be non-negative epoch millis with from <= to",
            ));
        }
        let in_window = |q: &graphdb_metrics::QueryProfile| {
            let ts = profile_start_ms(q);
            ts >= from_ms && ts <= to_ms
        };
        let stats_manager = self.app_state.server.get_stats_manager();
        let total = stats_manager
            .get_value(graphdb_metrics::MetricType::NumQueries)
            .unwrap_or(0) as i64;
        let slow: Vec<graphdb_metrics::QueryProfile> = stats_manager
            .get_slow_queries(10)
            .into_iter()
            .filter(in_window)
            .collect();
        let recent: Vec<graphdb_metrics::QueryProfile> = stats_manager
            .get_recent_queries(200)
            .into_iter()
            .filter(in_window)
            .collect();
        let failed = recent
            .iter()
            .filter(|q| q.status == graphdb_metrics::QueryStatus::Failed)
            .count() as i64;
        let (avg_ms, max_ms) = if recent.is_empty() {
            (0, 0)
        } else {
            let sum_ms: u64 = recent.iter().map(|q| q.total_duration_us / 1000).sum();
            let max_ms: u64 = recent
                .iter()
                .map(|q| q.total_duration_us / 1000)
                .max()
                .unwrap_or(0);
            ((sum_ms / recent.len() as u64) as i64, max_ms as i64)
        };
        Ok(Response::new(GetQueryStatisticsResponse {
            total_queries: total,
            slow_queries: slow.len() as i64,
            failed_queries: failed,
            avg_execution_time_ms: avg_ms,
            max_execution_time_ms: max_ms,
            slow_query_list: slow
                .into_iter()
                .map(|q| {
                    let timestamp = profile_start_ms(&q);
                    SlowQuery {
                        query: q.query_text,
                        execution_time_ms: (q.total_duration_us / 1000) as i64,
                        timestamp,
                        session_id: Some(q.session_id.to_string()),
                    }
                })
                .collect(),
        }))
    }

    async fn get_database_statistics(
        &self,
        _request: Request<GetDatabaseStatisticsRequest>,
    ) -> Result<Response<GetDatabaseStatisticsResponse>, Status> {
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        let stats = storage_guard.get_storage_stats();
        Ok(Response::new(GetDatabaseStatisticsResponse {
            total_spaces: stats.total_spaces as i32,
            total_vertices: stats.total_vertices as i64,
            total_edges: stats.total_edges as i64,
            storage_size_bytes: stats.total_size_bytes as i64,
        }))
    }

    async fn get_system_statistics(
        &self,
        _request: Request<GetSystemStatisticsRequest>,
    ) -> Result<Response<GetSystemStatisticsResponse>, Status> {
        let (memory_used, memory_total) = {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            (sys.used_memory() * 1024, sys.total_memory() * 1024)
        };
        let cpu_usage = {
            let mut sys = sysinfo::System::new();
            sys.refresh_cpu_usage();
            let cpus = sys.cpus();
            if cpus.is_empty() {
                0.0
            } else {
                let avg: f32 =
                    cpus.iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / cpus.len() as f32;
                avg as f64
            }
        };
        let active = self
            .app_state
            .server
            .get_session_manager()
            .active_session_count()
            .await as i32;
        let (disk_used_bytes, disk_total_bytes) = {
            let disks = sysinfo::Disks::new_with_refreshed_list();
            disks
                .list()
                .iter()
                .fold((0u64, 0u64), |(used, total), disk| {
                    (
                        used + disk.total_space().saturating_sub(disk.available_space()),
                        total + disk.total_space(),
                    )
                })
        };
        let disk_usage_percent = if disk_total_bytes > 0 {
            disk_used_bytes as f64 / disk_total_bytes as f64 * 100.0
        } else {
            0.0
        };
        // Cumulative interface counters since boot.
        let (network_rx_bytes, network_tx_bytes) = {
            let networks = sysinfo::Networks::new_with_refreshed_list();
            networks
                .list()
                .values()
                .fold((0u64, 0u64), |(rx, tx), data| {
                    (rx + data.received(), tx + data.transmitted())
                })
        };
        Ok(Response::new(GetSystemStatisticsResponse {
            cpu_usage_percent: cpu_usage,
            memory_used_bytes: memory_used as i64,
            memory_total_bytes: memory_total as i64,
            disk_usage_percent,
            active_connections: active,
            network_rx_bytes: network_rx_bytes as i64,
            network_tx_bytes: network_tx_bytes as i64,
        }))
    }

    // Configuration
    //
    // Reads serve the live configuration. Updates and resets are validated,
    // applied to live config, and persisted to the retained config file when
    // one exists; keys consumed from construction snapshots report
    // requires_restart.
    async fn get_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        let config = self.app_state.server.get_config();
        Ok(Response::new(GetConfigResponse {
            config: build_config_map(&config),
            error: String::new(),
        }))
    }

    async fn update_config(
        &self,
        request: Request<UpdateConfigRequest>,
    ) -> Result<Response<UpdateConfigResponse>, Status> {
        let req = request.into_inner();
        let value = proto_config_value_to_json(req.value);
        let store = self.app_state.server.config_store();
        let config_path = self.app_state.server.get_config_path();
        let (requires_restart, persisted) = crate::http::handlers::config::apply_config_update(
            &store,
            config_path.as_deref(),
            &req.section,
            &req.key,
            &value,
        )
        .map_err(Status::invalid_argument)?;
        Ok(Response::new(UpdateConfigResponse {
            success: true,
            error: String::new(),
            requires_restart,
            persisted,
        }))
    }

    async fn reset_config(
        &self,
        request: Request<ResetConfigRequest>,
    ) -> Result<Response<ResetConfigResponse>, Status> {
        let req = request.into_inner();
        // Reset funnels the default snapshot through the same typed
        // apply path as updates, so unknown keys fail identically.
        let default_config = crate::config::Config::default();
        let value = crate::http::handlers::config::get_config_value(
            &default_config,
            &req.section,
            &req.key,
        );
        let store = self.app_state.server.config_store();
        let config_path = self.app_state.server.get_config_path();
        let (requires_restart, persisted) = crate::http::handlers::config::apply_config_update(
            &store,
            config_path.as_deref(),
            &req.section,
            &req.key,
            &value,
        )
        .map_err(Status::invalid_argument)?;
        Ok(Response::new(ResetConfigResponse {
            success: true,
            error: String::new(),
            requires_restart,
            persisted,
        }))
    }

    // Custom Functions
    //
    // Registration loads the requested UDF library into the shared
    // registry, so a registered function is immediately listable and
    // executable. Only library-backed implementations can run.
    async fn register_function(
        &self,
        request: Request<RegisterFunctionRequest>,
    ) -> Result<Response<RegisterFunctionResponse>, Status> {
        use crate::http::handlers::function::register_udf_from_source;
        let req = request.into_inner();
        let registry = self.app_state.server.get_function_registry();
        let registered_name = register_udf_from_source(&registry, &req.name, &req.implementation)
            .map_err(op_error_to_status)?;
        Ok(Response::new(RegisterFunctionResponse {
            success: true,
            function_id: registered_name,
            error: String::new(),
        }))
    }

    async fn unregister_function(
        &self,
        request: Request<UnregisterFunctionRequest>,
    ) -> Result<Response<UnregisterFunctionResponse>, Status> {
        use crate::http::handlers::function::unregister_udf_by_name;
        let req = request.into_inner();
        let registry = self.app_state.server.get_function_registry();
        unregister_udf_by_name(&registry, &req.name).map_err(op_error_to_status)?;
        Ok(Response::new(UnregisterFunctionResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn list_functions(
        &self,
        _request: Request<ListFunctionsRequest>,
    ) -> Result<Response<ListFunctionsResponse>, Status> {
        let registry = self.app_state.server.get_function_registry();
        let registry_guard = registry.read();
        let functions = registry_guard
            .function_names()
            .into_iter()
            .map(|name| function_info_for(&registry_guard, name))
            .collect();
        Ok(Response::new(ListFunctionsResponse {
            functions,
            error: String::new(),
        }))
    }

    async fn get_function_info(
        &self,
        request: Request<GetFunctionInfoRequest>,
    ) -> Result<Response<GetFunctionInfoResponse>, Status> {
        let req = request.into_inner();
        let registry = self.app_state.server.get_function_registry();
        let registry_guard = registry.read();
        match registry_guard.contains(&req.name) {
            true => Ok(Response::new(GetFunctionInfoResponse {
                exists: true,
                function: Some(function_info_for(&registry_guard, &req.name)),
                error: String::new(),
            })),
            false => Ok(Response::new(GetFunctionInfoResponse {
                exists: false,
                function: None,
                error: String::new(),
            })),
        }
    }

    // Vector Index
    //
    // Transport mapping over the shared `VectorApi`: space names resolve to
    // ids through storage, indexes stay vertex-only like the HTTP path, and
    // option parsing mirrors the HTTP handler so both transports accept the
    // same parameters. Text queries are not supported here; the proto carries
    // an explicit vector only.
    async fn create_vector_index(
        &self,
        request: Request<CreateVectorIndexRequest>,
    ) -> Result<Response<CreateVectorIndexResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            if req.space_name.is_empty() || req.tag_name.is_empty() || req.field_name.is_empty() {
                return Err(Status::invalid_argument(
                    "space_name, tag_name and field_name are required",
                ));
            }
            let vector_api = self
                .app_state
                .server
                .get_graph_service()
                .vector_api()
                .cloned()
                .ok_or_else(|| Status::unavailable("vector API is not available"))?;
            let storage = self.app_state.server.get_storage();
            let (space_id, is_edge_type) = {
                let storage_read = storage.read();
                let space_id = storage_read.get_space_id(&req.space_name).map_err(|_| {
                    Status::not_found(format!("space '{}' not found", req.space_name))
                })?;
                let is_edge_type = storage_read
                    .get_edge_type(&req.space_name, &req.tag_name)
                    .map_err(|e| Status::internal(format!("failed to check tag type: {}", e)))?
                    .is_some();
                (space_id, is_edge_type)
            };
            if is_edge_type {
                return Err(Status::invalid_argument(format!(
                    "vector indexes are vertex-only: '{}' is an edge type",
                    req.tag_name
                )));
            }
            let config = grpc_collection_config(req.options)?;
            vector_api
                .create_index_with_config(space_id, &req.tag_name, &req.field_name, config)
                .await
                .map_err(|e| Status::internal(format!("failed to create vector index: {}", e)))?;
            Ok(Response::new(CreateVectorIndexResponse {
                success: true,
                error: String::new(),
            }))
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request.into_inner();
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    async fn get_vector_index(
        &self,
        request: Request<GetVectorIndexRequest>,
    ) -> Result<Response<GetVectorIndexResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            if req.space_name.is_empty() || req.tag_name.is_empty() || req.field_name.is_empty() {
                return Err(Status::invalid_argument(
                    "space_name, tag_name and field_name are required",
                ));
            }
            let vector_api = self
                .app_state
                .server
                .get_graph_service()
                .vector_api()
                .cloned()
                .ok_or_else(|| Status::unavailable("vector API is not available"))?;
            let storage = self.app_state.server.get_storage();
            let space_id = storage
                .read()
                .get_space_id(&req.space_name)
                .map_err(|_| Status::not_found(format!("space '{}' not found", req.space_name)))?;
            match vector_api.get_index_info(space_id, &req.tag_name, &req.field_name) {
                Err(e) => Err(Status::internal(format!(
                    "failed to get vector index: {}",
                    e
                ))),
                Ok(None) => Ok(Response::new(GetVectorIndexResponse {
                    exists: false,
                    index: None,
                    error: String::new(),
                })),
                Ok(Some(meta)) => Ok(Response::new(GetVectorIndexResponse {
                    exists: true,
                    index: Some(grpc_index_info_to_proto(
                        req.space_name,
                        req.tag_name,
                        req.field_name,
                        &meta,
                    )),
                    error: String::new(),
                })),
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request.into_inner();
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    async fn list_vector_indexes(
        &self,
        request: Request<ListVectorIndexesRequest>,
    ) -> Result<Response<ListVectorIndexesResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            let vector_api = self
                .app_state
                .server
                .get_graph_service()
                .vector_api()
                .cloned()
                .ok_or_else(|| Status::unavailable("vector API is not available"))?;
            let Some(coordinator) = vector_api.coordinator().cloned() else {
                return Ok(Response::new(ListVectorIndexesResponse {
                    indexes: Vec::new(),
                    error: String::new(),
                }));
            };
            let filter = req.space_name.filter(|name| !name.is_empty());
            let storage = self.app_state.server.get_storage();
            let storage_read = storage.read();
            let mut indexes = Vec::new();
            for wrapper in coordinator.list_indexes() {
                let space_name = match storage_read.get_space_by_id(wrapper.space_id) {
                    Ok(Some(info)) => info.space_name,
                    Ok(None) => continue,
                    Err(e) => {
                        return Err(Status::internal(format!(
                            "failed to resolve space name: {}",
                            e
                        )))
                    }
                };
                if let Some(wanted) = &filter {
                    if space_name != *wanted {
                        continue;
                    }
                }
                let Some(meta) = coordinator.index_info(
                    wrapper.space_id,
                    &wrapper.tag_name,
                    &wrapper.field_name,
                ) else {
                    continue;
                };
                indexes.push(grpc_index_info_to_proto(
                    space_name,
                    wrapper.tag_name,
                    wrapper.field_name,
                    &meta,
                ));
            }
            Ok(Response::new(ListVectorIndexesResponse {
                indexes,
                error: String::new(),
            }))
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request.into_inner();
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    async fn drop_vector_index(
        &self,
        request: Request<DropVectorIndexRequest>,
    ) -> Result<Response<DropVectorIndexResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            if req.space_name.is_empty() || req.tag_name.is_empty() || req.field_name.is_empty() {
                return Err(Status::invalid_argument(
                    "space_name, tag_name and field_name are required",
                ));
            }
            let vector_api = self
                .app_state
                .server
                .get_graph_service()
                .vector_api()
                .cloned()
                .ok_or_else(|| Status::unavailable("vector API is not available"))?;
            let storage = self.app_state.server.get_storage();
            let space_id = storage
                .read()
                .get_space_id(&req.space_name)
                .map_err(|_| Status::not_found(format!("space '{}' not found", req.space_name)))?;
            vector_api
                .drop_index(space_id, &req.tag_name, &req.field_name)
                .await
                .map_err(|e| Status::internal(format!("failed to drop vector index: {}", e)))?;
            Ok(Response::new(DropVectorIndexResponse {
                success: true,
                error: String::new(),
            }))
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request.into_inner();
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    async fn search_vector(
        &self,
        request: Request<SearchVectorRequest>,
    ) -> Result<Response<SearchVectorResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            if req.space_name.is_empty() || req.tag_name.is_empty() || req.field_name.is_empty() {
                return Err(Status::invalid_argument(
                    "space_name, tag_name and field_name are required",
                ));
            }
            if req.vector.is_empty() {
                return Err(Status::invalid_argument("query vector must not be empty"));
            }
            if req.limit <= 0 {
                return Err(Status::invalid_argument("limit must be greater than 0"));
            }
            // `ef_search` only tunes HNSW recall, never result correctness, so
            // it stays on server defaults until the search options carry it.
            let with_vector = req.options.as_ref().is_some_and(|o| o.with_vector);
            let vector_api = self
                .app_state
                .server
                .get_graph_service()
                .vector_api()
                .cloned()
                .ok_or_else(|| Status::unavailable("vector API is not available"))?;
            let storage = self.app_state.server.get_storage();
            let space_id = storage
                .read()
                .get_space_id(&req.space_name)
                .map_err(|_| Status::not_found(format!("space '{}' not found", req.space_name)))?;
            let mut options = graphdb_sync::vector_sync::SearchOptions::new(
                space_id,
                req.tag_name,
                req.field_name,
                req.vector,
                req.limit as usize,
            );
            if let Some(filter) = &req.filter {
                if !filter.expression.is_empty() {
                    let parsed = crate::http::handlers::vector::parse_vector_filter_expression(
                        &filter.expression,
                    )
                    .map_err(Status::invalid_argument)?;
                    options = options.with_filter(parsed);
                }
            }
            let results = vector_api
                .search_with_options(options)
                .await
                .map_err(|e| Status::internal(format!("vector search failed: {}", e)))?;
            let proto_results = results
                .into_iter()
                .map(|r| {
                    let properties = r
                        .payload
                        .unwrap_or_default()
                        .iter()
                        .map(|(k, v)| (k.clone(), value_to_proto_value(crate::value::from_json(v))))
                        .collect();
                    super::proto::VectorSearchResult {
                        vid: r.id.to_string(),
                        score: r.score,
                        properties,
                        vector: if with_vector {
                            r.vector.unwrap_or_default()
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect();
            Ok(Response::new(SearchVectorResponse {
                results: proto_results,
                error: String::new(),
            }))
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request.into_inner();
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    async fn get_version_history(
        &self,
        request: Request<VersionHistoryRequest>,
    ) -> Result<Response<VersionHistoryResponse>, Status> {
        let req = request.into_inner();

        let storage = self.app_state.server.get_storage();
        let storage_read = storage.read();

        let history = if req.is_edge {
            storage_read
                .get_edge_version_history(&req.space, &req.label)
                .map_err(|e| {
                    Status::internal(format!("Failed to get edge version history: {}", e))
                })?
        } else {
            storage_read
                .get_vertex_version_history(&req.space, &req.label)
                .map_err(|e| {
                    Status::internal(format!("Failed to get vertex version history: {}", e))
                })?
        };

        let versions = history
            .map(|h| {
                h.change_log
                    .get_versions()
                    .iter()
                    .map(|&version| {
                        let version_changes = h
                            .change_log
                            .get_version_changes(version)
                            .cloned()
                            .unwrap_or_default();
                        let timestamp_ms = version_changes
                            .iter()
                            .map(|c| c.timestamp_ms)
                            .max()
                            .unwrap_or(0) as i64;
                        let changes = version_changes
                            .into_iter()
                            .map(|change| PropertyChangeEvent {
                                change_type: format!("{:?}", change.details),
                                details: {
                                    let mut details = std::collections::HashMap::new();
                                    details.insert(
                                        "description".to_string(),
                                        change.details.description(),
                                    );
                                    details
                                        .insert("version".to_string(), change.version.to_string());
                                    details
                                },
                            })
                            .collect();

                        SchemaVersion {
                            version,
                            timestamp_ms,
                            changes,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Response::new(VersionHistoryResponse {
            versions,
            error: String::new(),
        }))
    }

    async fn get_schema_changes(
        &self,
        request: Request<SchemaChangesRequest>,
    ) -> Result<Response<SchemaChangesResponse>, Status> {
        let req = request.into_inner();

        // Validate version range: from_version must be <= to_version
        if req.from_version > req.to_version {
            return Err(Status::invalid_argument(format!(
                "Invalid version range: from_version ({}) must be <= to_version ({})",
                req.from_version, req.to_version
            )));
        }

        let storage = self.app_state.server.get_storage();
        let storage_read = storage.read();

        let changes = if req.is_edge {
            storage_read
                .get_edge_schema_changes(&req.space, &req.label, req.from_version, req.to_version)
                .map_err(|e| {
                    Status::internal(format!("Failed to get edge schema changes: {}", e))
                })?
        } else {
            storage_read
                .get_vertex_schema_changes(&req.space, &req.label, req.from_version, req.to_version)
                .map_err(|e| {
                    Status::internal(format!("Failed to get vertex schema changes: {}", e))
                })?
        };

        let proto_changes = changes
            .iter()
            .map(|change| PropertyChangeEvent {
                change_type: format!("{:?}", change.details),
                details: {
                    let mut details = std::collections::HashMap::new();
                    details.insert("description".to_string(), change.details.description());
                    details.insert("version".to_string(), change.version.to_string());
                    details
                },
            })
            .collect();

        Ok(Response::new(SchemaChangesResponse {
            changes: proto_changes,
            error: String::new(),
        }))
    }

    async fn detect_breaking_changes(
        &self,
        request: Request<BreakingChangesRequest>,
    ) -> Result<Response<BreakingChangesResponse>, Status> {
        let req = request.into_inner();

        // Validate version range: from_version must be <= to_version
        if req.from_version > req.to_version {
            return Err(Status::invalid_argument(format!(
                "Invalid version range: from_version ({}) must be <= to_version ({})",
                req.from_version, req.to_version
            )));
        }

        let storage = self.app_state.server.get_storage();
        let storage_read = storage.read();

        let changes = if req.is_edge {
            storage_read
                .detect_edge_breaking_changes(
                    &req.space,
                    &req.label,
                    req.from_version,
                    req.to_version,
                )
                .map_err(|e| {
                    Status::internal(format!("Failed to detect edge breaking changes: {}", e))
                })?
        } else {
            storage_read
                .detect_vertex_breaking_changes(
                    &req.space,
                    &req.label,
                    req.from_version,
                    req.to_version,
                )
                .map_err(|e| {
                    Status::internal(format!("Failed to detect vertex breaking changes: {}", e))
                })?
        };

        let has_breaking = !changes.is_empty();
        let proto_changes: Vec<PropertyChangeEvent> = changes
            .iter()
            .map(|change| PropertyChangeEvent {
                change_type: format!("{:?}", change.details),
                details: {
                    let mut details = std::collections::HashMap::new();
                    details.insert("description".to_string(), change.details.description());
                    details.insert("version".to_string(), change.version.to_string());
                    details
                },
            })
            .collect();

        let recommendation = if has_breaking {
            format!(
                "Found {} breaking changes. Data migration may be required.",
                proto_changes.len()
            )
        } else {
            "No breaking changes detected".to_string()
        };

        Ok(Response::new(BreakingChangesResponse {
            has_breaking_changes: has_breaking,
            changes: proto_changes,
            recommendation,
            error: String::new(),
        }))
    }

    async fn migrate_plan(
        &self,
        request: Request<MigratePlanRequest>,
    ) -> Result<Response<MigratePlanResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_read = storage.read();

        let plan = if req.is_edge {
            graphdb_migration::generate_edge_plan(
                &*storage_read,
                &req.space,
                &req.label,
                req.from_version,
                req.to_version,
            )
        } else {
            graphdb_migration::generate_vertex_plan(
                &*storage_read,
                &req.space,
                &req.label,
                req.from_version,
                req.to_version,
            )
        }
        .map_err(|e| Status::internal(e.to_string()))?;

        let plan_json =
            serde_json::to_string(&plan).map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(MigratePlanResponse {
            plan_json,
            safety_level: format!("{:?}", plan.overall_safety),
            estimated_rows: plan.estimated_rows,
            steps: plan
                .steps
                .iter()
                .map(|s| MigrationStep {
                    step_type: format!("{:?}", s),
                    description: s.description(),
                    safety_level: format!("{:?}", s.safety_level()),
                    is_data_modifying: s.is_data_modifying(),
                })
                .collect(),
            error: String::new(),
        }))
    }

    async fn migrate_execute(
        &self,
        request: Request<MigrateExecuteRequest>,
    ) -> Result<Response<MigrateExecuteResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let stats = self.app_state.server.get_stats_manager();
        let start = std::time::Instant::now();
        stats.record_migration_start();
        let mut storage_write = storage.write();

        let plan: graphdb_migration::MigrationPlan = serde_json::from_str(&req.plan_json)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let listener = crate::http::handlers::migration_progress::BroadcastEventListener::new(
            &plan.target.space,
            &plan.target.label,
            plan.target.is_edge,
        );
        let report = graphdb_migration::execute_migration_plan_with_progress(
            &mut *storage_write,
            &plan,
            &graphdb_migration::NoopProgress,
            Some(&listener),
        );
        let elapsed = start.elapsed().as_millis() as u64;
        match &report {
            Ok(r) if r.success => stats.record_migration_success(r.rows_migrated, elapsed),
            Ok(_) => stats.record_migration_failure(elapsed),
            Err(_) => stats.record_migration_failure(elapsed),
        }
        let report = report.map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(MigrateExecuteResponse {
            success: report.success,
            steps_completed: report.steps_completed as u64,
            rows_migrated: report.rows_migrated,
            errors: report.errors,
            error: String::new(),
        }))
    }

    async fn migrate_rollback(
        &self,
        request: Request<MigrateRollbackRequest>,
    ) -> Result<Response<MigrateRollbackResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let mut storage_write = storage.write();

        let plan: graphdb_migration::MigrationPlan = serde_json::from_str(&req.plan_json)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let report = graphdb_migration::rollback_migration(&mut *storage_write, &plan)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(MigrateRollbackResponse {
            success: report.success,
            steps_completed: report.steps_completed as u64,
            rows_migrated: report.rows_migrated,
            errors: report.errors,
            error: String::new(),
        }))
    }

    async fn stream_migration_progress(
        &self,
        request: Request<StreamMigrationProgressRequest>,
    ) -> Result<Response<Self::StreamMigrationProgressStream>, Status> {
        let req = request.into_inner();
        let rx = crate::http::handlers::migration_progress::subscribe(
            &req.space,
            &req.label,
            req.is_edge,
        );
        let rx_stream = tokio_stream::wrappers::BroadcastStream::new(rx);
        let stream = rx_stream.filter_map(|res| match res {
            Ok(ev) => {
                let proto_ev = match ev {
                    graphdb_migration::MigrationEvent::Started { plan } => MigrationProgressEvent {
                        event_type: "started".to_string(),
                        message: plan.plan_hash.clone(),
                        step_idx: 0,
                        rows: 0,
                        success: false,
                        error: String::new(),
                    },
                    graphdb_migration::MigrationEvent::StepStarted { step_idx } => {
                        MigrationProgressEvent {
                            event_type: "step_started".to_string(),
                            message: String::new(),
                            step_idx: step_idx as u64,
                            rows: 0,
                            success: false,
                            error: String::new(),
                        }
                    }
                    graphdb_migration::MigrationEvent::StepCompleted { step_idx, rows } => {
                        MigrationProgressEvent {
                            event_type: "step_completed".to_string(),
                            message: String::new(),
                            step_idx: step_idx as u64,
                            rows,
                            success: true,
                            error: String::new(),
                        }
                    }
                    graphdb_migration::MigrationEvent::Completed { report } => {
                        MigrationProgressEvent {
                            event_type: "completed".to_string(),
                            message: format!(
                                "{} steps, {} rows",
                                report.steps_completed, report.rows_migrated
                            ),
                            step_idx: 0,
                            rows: report.rows_migrated,
                            success: report.success,
                            error: report.errors.join("; "),
                        }
                    }
                    graphdb_migration::MigrationEvent::Failed { error } => MigrationProgressEvent {
                        event_type: "failed".to_string(),
                        message: String::new(),
                        step_idx: 0,
                        rows: 0,
                        success: false,
                        error,
                    },
                    graphdb_migration::MigrationEvent::RolledBack { report } => {
                        MigrationProgressEvent {
                            event_type: "rolled_back".to_string(),
                            message: String::new(),
                            step_idx: 0,
                            rows: report.rows_migrated,
                            success: report.success,
                            error: String::new(),
                        }
                    }
                };
                Some(Ok(proto_ev))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => None,
        });
        let boxed: Self::StreamMigrationProgressStream = Box::pin(stream);
        Ok(Response::new(boxed))
    }
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > GraphDBService<S>
{
}

/// Run the gRPC server
pub async fn run_server<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + StorageSnapshotOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    app_state: AppState<S>,
    config: Config,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = GraphDBService::new(app_state.clone(), config.clone());

    tracing::info!("GraphDB gRPC service listening on {}", addr);

    let grpc_cfg = config.grpc().clone();
    let builder = Server::builder();
    let builder = if grpc_cfg.request_timeout_secs > 0 {
        builder.timeout(std::time::Duration::from_secs(
            grpc_cfg.request_timeout_secs,
        ))
    } else {
        builder
    };
    let builder = if grpc_cfg.keepalive_interval_secs > 0 {
        builder.http2_keepalive_interval(Some(std::time::Duration::from_secs(
            grpc_cfg.keepalive_interval_secs,
        )))
    } else {
        builder
    };
    let mut builder = if grpc_cfg.keepalive_timeout_secs > 0 {
        builder.http2_keepalive_timeout(Some(std::time::Duration::from_secs(
            grpc_cfg.keepalive_timeout_secs,
        )))
    } else {
        builder
    };
    let router = builder.add_service(
        GraphDbServiceServer::new(service)
            .max_decoding_message_size(grpc_cfg.max_request_size)
            .max_encoding_message_size(grpc_cfg.max_response_size),
    );

    router.serve(addr).await?;

    Ok(())
}

/// Run the gRPC server with custom service instance
pub async fn run_server_with_grpc_service<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    service: GraphDBService<S>,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("GraphDB gRPC service listening on {}", addr);

    Server::builder()
        .add_service(GraphDbServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}

/// Parse a string session id carried on the wire into the numeric id.
fn parse_session_id(value: &str) -> Result<i64, Status> {
    value
        .parse::<i64>()
        .map_err(|_| Status::invalid_argument("session_id must be an integer"))
}

/// Seconds since the Unix epoch for a `SystemTime`, saturating at zero.
fn system_time_secs(time: &SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Wall-clock start of a query profile in epoch millis.
///
/// Profiles only record a monotonic `Instant`; wall time is derived as
/// now minus elapsed, which is exact up to clock adjustments.
fn profile_start_ms(profile: &graphdb_metrics::QueryProfile) -> i64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    now_ms.saturating_sub(profile.start_time.elapsed().as_millis() as i64)
}

/// Attach an authenticated session to a space by name.
fn attach_session_space<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + 'static,
>(
    app_state: &AppState<S>,
    session: &std::sync::Arc<crate::client::ClientSession>,
    space: &str,
) -> Result<(), Status> {
    let storage = app_state.server.get_storage();
    let info = storage
        .read()
        .get_space(space)
        .map_err(|e| Status::internal(format!("failed to resolve space: {e}")))?
        .ok_or_else(|| Status::not_found(format!("space '{space}' not found")))?;
    session.set_space(graphdb_core::types::SpaceSummary::new(
        info.space_id,
        info.space_name,
        info.vid_type,
    ));
    Ok(())
}

/// Convert a proto `Value` to a core `Value`.
///
/// The wire value vocabulary is intentionally narrow; complex core types
/// have no proto spelling and never appear here. Timestamps cross the wire
/// as epoch millis and stay `BigInt` so no timezone interpretation is
/// smuggled in.
fn proto_value_to_core(value: super::proto::Value) -> graphdb_core::Value {
    use super::proto::value::Value as ProtoValue;
    match value.value {
        None => graphdb_core::Value::Empty,
        Some(ProtoValue::StringValue(s)) => graphdb_core::Value::string(s),
        Some(ProtoValue::IntValue(i)) => {
            if i >= i64::from(i32::MIN) && i <= i64::from(i32::MAX) {
                graphdb_core::Value::Int(i as i32)
            } else {
                graphdb_core::Value::BigInt(i)
            }
        }
        Some(ProtoValue::DoubleValue(d)) => graphdb_core::Value::Double(d),
        Some(ProtoValue::FloatValue(f)) => graphdb_core::Value::Float(f as f32),
        Some(ProtoValue::BoolValue(b)) => graphdb_core::Value::Bool(b),
        Some(ProtoValue::BytesValue(b)) => graphdb_core::Value::Blob(b),
        Some(ProtoValue::TimestampValue(t)) => graphdb_core::Value::BigInt(t),
    }
}

/// Render a core `QueryResult` into the proto result/metadata pair.
///
/// Rows come from the engine `ExecutionResult` unchanged (column order
/// preserved); non-dataset results yield empty columns and rows.
fn query_result_to_proto(
    result: &graphdb_api::api_core::QueryResult,
) -> (
    Option<super::proto::QueryResult>,
    Option<super::proto::ExecutionMetadata>,
) {
    let rows: Vec<super::proto::Row> = result
        .rows()
        .iter()
        .map(|row| super::proto::Row {
            values: row.iter().cloned().map(value_to_proto_value).collect(),
        })
        .collect();
    let query_result = super::proto::QueryResult {
        column_names: result.columns().to_vec(),
        rows,
        plan_descriptions: HashMap::new(),
    };
    let metadata = super::proto::ExecutionMetadata {
        rows_returned: result.rows().len() as u64,
        execution_time_ms: result.metadata.execution_time_ms,
        rows_scanned: result.metadata.rows_scanned,
        custom_stats: HashMap::new(),
    };
    (Some(query_result), Some(metadata))
}

fn core_space_to_proto(info: &graphdb_core::types::SpaceInfo) -> super::proto::SpaceInfo {
    super::proto::SpaceInfo {
        id: info.space_id as i32,
        name: info.space_name.clone(),
        options: Some(super::proto::SpaceOptions {
            partition_num: info.partition_num,
            replica_num: info.replica_factor,
            charset: String::new(),
            collate: String::new(),
            vid_fixed_length: false,
            vid_length: 0,
        }),
        created_at: 0,
    }
}

fn core_tag_to_proto(info: &graphdb_core::types::TagInfo) -> super::proto::TagInfo {
    super::proto::TagInfo {
        id: info.tag_id as i32,
        name: info.tag_name.clone(),
        properties: info.properties.iter().map(core_property_to_proto).collect(),
        options: Some(super::proto::TagOptions {
            ttl_seconds: info.ttl_duration.unwrap_or(0),
            ttl_column: info.ttl_col.clone().unwrap_or_default(),
        }),
        created_at: 0,
    }
}

/// Map a proto batch item onto the wire batch item.
///
/// Inserts run through the core batch operation while updates and deletes
/// run as direct storage mutations in the batch manager; all variants have
/// a wire representation and are reported with per-item error indexes.
fn proto_batch_item_to_wire(
    item: super::proto::BatchItem,
) -> Result<crate::batch::BatchItem, Status> {
    use super::proto::batch_item::Operation;
    let operation = item
        .operation
        .ok_or_else(|| Status::invalid_argument("batch item operation is required"))?;
    match operation {
        Operation::InsertVertex(v) => {
            Ok(crate::batch::BatchItem::Vertex(crate::batch::VertexData {
                vid: serde_json::Value::String(v.vid),
                tag: v.tag_name,
                properties: proto_properties_to_json(v.properties),
            }))
        }
        Operation::InsertEdge(e) => Ok(crate::batch::BatchItem::Edge(crate::batch::EdgeData {
            edge_type: e.edge_type,
            src_vid: serde_json::Value::String(e.src),
            dst_vid: serde_json::Value::String(e.dst),
            properties: proto_properties_to_json(e.properties),
        })),
        Operation::UpdateVertex(v) => Ok(crate::batch::BatchItem::UpdateVertex(
            crate::batch::VertexData {
                vid: serde_json::Value::String(v.vid),
                tag: v.tag_name,
                properties: proto_properties_to_json(v.properties),
            },
        )),
        Operation::UpdateEdge(e) => Ok(crate::batch::BatchItem::UpdateEdgeData(
            crate::batch::UpdateEdgeData {
                edge_type: e.edge_type,
                src_vid: serde_json::Value::String(e.src),
                dst_vid: serde_json::Value::String(e.dst),
                rank: e.ranking,
                properties: proto_properties_to_json(e.properties),
            },
        )),
        Operation::DeleteVertex(v) => Ok(crate::batch::BatchItem::DeleteVertex(
            crate::batch::DeleteVertexData {
                vid: serde_json::Value::String(v.vid),
                tag_names: v.tag_names,
            },
        )),
        Operation::DeleteEdge(e) => Ok(crate::batch::BatchItem::DeleteEdgeData(
            crate::batch::DeleteEdgeData {
                edge_type: e.edge_type,
                src_vid: serde_json::Value::String(e.src),
                dst_vid: serde_json::Value::String(e.dst),
                rank: e.ranking,
            },
        )),
    }
}

fn proto_properties_to_json(
    properties: HashMap<String, super::proto::Value>,
) -> HashMap<String, serde_json::Value> {
    properties
        .into_iter()
        .map(|(k, v)| (k, crate::value::to_json(proto_value_to_core(v))))
        .collect()
}

/// Render the internal batch status with its proto spelling.
fn batch_status_name(status: &crate::batch::BatchStatus) -> String {
    use crate::batch::BatchStatus;
    match status {
        BatchStatus::Created => "PENDING",
        BatchStatus::Running => "RUNNING",
        BatchStatus::Completed => "COMPLETED",
        BatchStatus::Failed => "FAILED",
        BatchStatus::Cancelled => "CANCELLED",
    }
    .to_string()
}

/// Build the proto config map from the live server configuration.
///
/// The section/key shape mirrors the HTTP config endpoint; values convert
/// through JSON so numeric widths and enums never need manual casting.
fn build_config_map(
    config: &crate::config::Config,
) -> HashMap<String, super::proto::ConfigSection> {
    let snapshot = serde_json::json!({
        "database": {
            "host": config.common.database.host,
            "port": config.common.database.port,
            "storage_path": config.common.database.storage_path,
            "max_connections": config.common.database.max_connections,
        },
        "transaction": {
            "default_timeout": config.common.transaction.default_timeout,
            "max_concurrent_transactions": config.common.transaction.max_concurrent_transactions,
            "auto_commit": config.common.transaction.auto_commit,
        },
        "log": {
            "level": config.common.log.level,
            "dir": config.common.log.dir,
            "file": config.common.log.file,
            "max_file_size": config.common.log.max_file_size,
            "max_files": config.common.log.max_files,
        },
        "auth": {
            "enable_authorize": config.server.auth.enable_authorize,
            "failed_login_attempts": config.server.auth.failed_login_attempts,
            "session_idle_timeout_secs": config.server.auth.session_idle_timeout_secs,
            "force_change_default_password": config.server.auth.force_change_default_password,
            "default_username": config.server.auth.default_username,
            "bcrypt_cost": config.server.auth.bcrypt_cost,
        },
        "bootstrap": {
            "auto_create_default_space": config.server.bootstrap.auto_create_default_space,
            "default_space_name": config.server.bootstrap.default_space_name,
            "single_user_mode": config.server.bootstrap.single_user_mode,
        },
        "optimizer": {
            "max_iteration_rounds": config.common.optimizer.max_iteration_rounds,
            "max_exploration_rounds": config.common.optimizer.max_exploration_rounds,
            "enable_cost_model": config.common.optimizer.enable_cost_model,
            "enable_multi_plan": config.common.optimizer.enable_multi_plan,
            "enable_property_pruning": config.common.optimizer.enable_property_pruning,
            "enable_adaptive_iteration": config.common.optimizer.enable_adaptive_iteration,
            "stable_threshold": config.common.optimizer.stable_threshold,
            "min_iteration_rounds": config.common.optimizer.min_iteration_rounds,
            "statistics_sample_limit": config.common.optimizer.statistics_sample_limit,
            "statistics_min_epoch_delta": config.common.optimizer.statistics_min_epoch_delta,
            "storage_cost_profile": format!("{:?}", config.common.optimizer.storage_cost_profile),
            "space_cost_profiles": format!("{:?}", config.common.optimizer.space_cost_profiles),
        },
        "monitoring": {
            "enabled": config.common.monitoring.enabled,
            "memory_cache_size": config.common.monitoring.memory_cache_size,
            "slow_query_threshold_ms": config.common.monitoring.slow_query_threshold_ms,
        },
    });
    snapshot
        .as_object()
        .map(|sections| {
            sections
                .iter()
                .map(|(section, values)| {
                    let entries = values
                        .as_object()
                        .map(|keys| {
                            keys.iter()
                                .map(|(key, value)| (key.clone(), json_to_config_value(value)))
                                .collect()
                        })
                        .unwrap_or_default();
                    (
                        section.clone(),
                        super::proto::ConfigSection { values: entries },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn json_to_config_value(value: &serde_json::Value) -> super::proto::ConfigValue {
    use super::proto::config_value::Value as ConfigPrimitive;
    let primitive = match value {
        serde_json::Value::String(s) => Some(ConfigPrimitive::StringValue(s.clone())),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(ConfigPrimitive::IntValue(i))
            } else if let Some(u) = n.as_u64() {
                Some(ConfigPrimitive::IntValue(u as i64))
            } else {
                n.as_f64().map(ConfigPrimitive::DoubleValue)
            }
        }
        serde_json::Value::Bool(b) => Some(ConfigPrimitive::BoolValue(*b)),
        _ => None,
    };
    super::proto::ConfigValue { value: primitive }
}

/// Proto config value back to JSON for the typed config setter.
fn proto_config_value_to_json(value: Option<super::proto::ConfigValue>) -> serde_json::Value {
    use super::proto::config_value::Value as ConfigPrimitive;
    match value.and_then(|v| v.value) {
        Some(ConfigPrimitive::StringValue(s)) => serde_json::Value::String(s),
        Some(ConfigPrimitive::IntValue(i)) => serde_json::Value::from(i),
        Some(ConfigPrimitive::DoubleValue(f)) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Some(ConfigPrimitive::BoolValue(b)) => serde_json::Value::Bool(b),
        None => serde_json::Value::Null,
    }
}

/// Describe a registered function for the wire.
//
// Descriptions and return types come from the registry; parameter names
// are not modeled anywhere, so the list stays empty instead of carrying
// invented names.
fn op_error_to_status(error: crate::http::handlers::function::FunctionOpError) -> Status {
    use crate::http::handlers::function::FunctionOpError;
    match error {
        FunctionOpError::NotFound(message) => Status::not_found(message),
        FunctionOpError::Conflict(message) => Status::already_exists(message),
        FunctionOpError::Invalid(message) => Status::invalid_argument(message),
        FunctionOpError::Failed(message) => Status::internal(message),
    }
}
fn function_info_for(
    registry: &crate::query::executor::expression::functions::FunctionRegistry,
    name: &str,
) -> super::proto::FunctionInfo {
    let builtin = registry.get_builtin(name);
    let custom = registry.get_custom(name);
    let (function_type, description) = match (&builtin, &custom) {
        (Some(function), _) => ("builtin", function.description().to_string()),
        (None, Some(function)) => {
            let arity = if function.is_variadic {
                format!("variadic from {}", function.arity)
            } else {
                format!("arity {}", function.arity)
            };
            (
                "custom",
                if function.description.is_empty() {
                    arity
                } else {
                    format!("{} ({arity})", function.description)
                },
            )
        }
        (None, None) => ("unknown", String::new()),
    };
    super::proto::FunctionInfo {
        name: name.to_string(),
        function_type: function_type.to_string(),
        parameters: vec![],
        return_type: registry
            .get_return_type(name)
            .map(|t| format!("{t:?}"))
            .unwrap_or_else(|| "unknown".to_string()),
        description,
    }
}

#[allow(clippy::result_large_err)]
fn parse_transaction_id(value: &str) -> Result<TransactionId, Status> {
    value
        .parse::<u64>()
        .map(TransactionId::from)
        .map_err(|_| Status::invalid_argument("transaction_id must be an unsigned integer"))
}

fn transaction_status(error: TransactionError) -> Status {
    let message = error.to_string();
    match error.kind() {
        TransactionErrorKind::TransactionNotFound => Status::not_found(message),
        TransactionErrorKind::TransactionNotOwner => Status::permission_denied(message),
        TransactionErrorKind::TransactionTimeout | TransactionErrorKind::TransactionExpired => {
            Status::deadline_exceeded(message)
        }
        TransactionErrorKind::WriteTransactionConflict => Status::aborted(message),
        TransactionErrorKind::CommitVetoed => Status::aborted(message),
        TransactionErrorKind::InvalidStateForCommit
        | TransactionErrorKind::InvalidStateForAbort
        | TransactionErrorKind::InvalidStateForExecution
        | TransactionErrorKind::InvalidStateTransition => Status::failed_precondition(message),
        TransactionErrorKind::SavepointNotFound
        | TransactionErrorKind::SavepointFailed
        | TransactionErrorKind::SavepointNotActive
        | TransactionErrorKind::NoSavepointsInTransaction => Status::failed_precondition(message),
        _ => Status::internal(message),
    }
}

fn proto_property_type_to_data_type(value: i32) -> graphdb_core::DataType {
    use graphdb_core::DataType;
    match value {
        0 => DataType::Bool,
        1 => DataType::Int,
        2 => DataType::Float,
        3 => DataType::Double,
        4 => DataType::String,
        5 | 7 => DataType::DateTime,
        6 => DataType::Date,
        8 => DataType::String,
        9 => DataType::Edge,
        10 => DataType::Vertex,
        11 => DataType::List(Box::new(DataType::Empty)),
        12 => DataType::Set(Box::new(DataType::Empty)),
        13 => DataType::Map(Box::new(DataType::Empty)),
        _ => DataType::String,
    }
}

fn data_type_to_proto_property_type(data_type: &graphdb_core::DataType) -> i32 {
    use graphdb_core::DataType;
    match data_type {
        DataType::Bool => 0,
        DataType::Int | DataType::SmallInt | DataType::BigInt => 1,
        DataType::Float => 2,
        DataType::Double => 3,
        DataType::String | DataType::FixedString(_) => 4,
        DataType::DateTime => 7,
        DataType::Date => 6,
        DataType::Time => 5,
        DataType::Edge => 9,
        DataType::Vertex => 10,
        DataType::List(_) => 11,
        DataType::Set(_) => 12,
        DataType::Map(_) => 13,
        _ => 4,
    }
}

fn proto_property_to_core(prop: super::proto::PropertyDef) -> graphdb_core::types::PropertyDef {
    graphdb_core::types::PropertyDef::new(prop.name, proto_property_type_to_data_type(prop.r#type))
        .with_nullable(prop.nullable)
}

fn core_property_to_proto(prop: &graphdb_core::types::PropertyDef) -> super::proto::PropertyDef {
    super::proto::PropertyDef {
        name: prop.name.clone(),
        r#type: data_type_to_proto_property_type(&prop.data_type),
        nullable: prop.nullable,
        default_value: None,
        is_primary_key: false,
    }
}

fn core_edge_info_to_proto(info: &graphdb_core::types::EdgeTypeInfo) -> super::proto::EdgeTypeInfo {
    super::proto::EdgeTypeInfo {
        id: info.edge_type_id as i32,
        name: info.edge_type_name.clone(),
        properties: info.properties.iter().map(core_property_to_proto).collect(),
        options: Some(super::proto::EdgeTypeOptions {
            directed: true,
            ttl_seconds: info.ttl_duration.unwrap_or(0),
            ttl_column: info.ttl_col.clone().unwrap_or_default(),
        }),
        created_at: 0,
    }
}

/// Map a proto `DistanceMetric` discriminant to the local metric.
///
/// Only Cosine, L2 and Dot exist on the wire; anything else is rejected so a
/// caller can never silently create an index with a different metric.
#[cfg(feature = "vector")]
fn proto_metric_to_distance(metric: i32) -> Result<vector_search::DistanceMetric, Status> {
    use vector_search::DistanceMetric;
    match metric {
        0 => Ok(DistanceMetric::Cosine),
        1 => Ok(DistanceMetric::Euclid),
        2 => Ok(DistanceMetric::Dot),
        other => Err(Status::invalid_argument(format!(
            "unknown distance metric '{}', expected 0 (cosine), 1 (l2) or 2 (dot)",
            other
        ))),
    }
}

/// Map a local metric back to its proto discriminant.
///
/// The wire enum has no Manhattan variant; it is reported as an
/// out-of-range sentinel so readers never mistake it for another metric.
#[cfg(feature = "vector")]
fn distance_to_proto_metric(metric: vector_search::DistanceMetric) -> i32 {
    use vector_search::DistanceMetric;
    match metric {
        DistanceMetric::Cosine => 0,
        DistanceMetric::Euclid => 1,
        DistanceMetric::Dot => 2,
        DistanceMetric::Manhattan => 3,
    }
}

/// Parse an optional `parameters` entry with a plain-text error.
#[cfg(feature = "vector")]
fn parse_optional_param<T>(
    params: &std::collections::HashMap<String, String>,
    key: &str,
) -> Result<Option<T>, Status>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match params.get(key) {
        None => Ok(None),
        Some(raw) => raw.parse::<T>().map(Some).map_err(|e| {
            Status::invalid_argument(format!("parameter '{}' is invalid: {}", key, e))
        }),
    }
}

/// Build a collection config from proto index options.
///
/// Accepts the same parameter keys as the HTTP creation endpoint
/// (`hnsw_m`, `hnsw_ef_construct`, `quantization`, `quantile`,
/// `compression`, `always_ram`) so both transports configure indexes
/// identically.
#[cfg(feature = "vector")]
fn grpc_collection_config(
    options: Option<super::proto::VectorIndexOptions>,
) -> Result<vector_search::CollectionConfig, Status> {
    use vector_search::{CollectionConfig, CompressionRatio, HnswConfig, IndexType};
    let opts = options.ok_or_else(|| Status::invalid_argument("index options are required"))?;
    if opts.dimension <= 0 {
        return Err(Status::invalid_argument("dimension must be greater than 0"));
    }
    let distance = proto_metric_to_distance(opts.metric)?;
    let mut config = CollectionConfig::new(opts.dimension as usize, distance);
    let kind = opts.index_type.trim().to_uppercase();
    match kind.as_str() {
        "" | "HNSW" => {}
        "FLAT" => {
            config = config.with_index_type(IndexType::FLAT);
        }
        "IVF" => {
            config = config.with_index_type(IndexType::IVF);
        }
        other => {
            return Err(Status::invalid_argument(format!(
                "unknown index_type '{}', expected HNSW, FLAT or IVF",
                other
            )))
        }
    }
    let params = &opts.parameters;
    let hnsw_m = parse_optional_param::<usize>(params, "hnsw_m")?;
    let hnsw_ef = parse_optional_param::<usize>(params, "hnsw_ef_construct")?;
    if hnsw_m.is_some() || hnsw_ef.is_some() {
        if kind.as_str() == "FLAT" || kind.as_str() == "IVF" {
            return Err(Status::invalid_argument(
                "hnsw parameters require index_type HNSW",
            ));
        }
        let mut hnsw = HnswConfig::default();
        if let Some(m) = hnsw_m {
            hnsw.m = m;
        }
        if let Some(ef) = hnsw_ef {
            hnsw.ef_construct = ef;
        }
        config = config.with_hnsw(hnsw);
    }
    if let Some(quantization) = params.get("quantization") {
        match quantization.to_lowercase().as_str() {
            "none" | "disabled" | "off" => {}
            "scalar" => {
                let quantile = parse_optional_param::<f32>(params, "quantile")?.unwrap_or(0.99);
                let mut cfg = vector_search::QuantizationConfig::scalar(quantile);
                if let Some(always_ram) = parse_optional_param::<bool>(params, "always_ram")? {
                    cfg = cfg.with_always_ram(always_ram);
                }
                config = config.with_quantization(cfg);
            }
            "binary" => {
                let mut cfg = vector_search::QuantizationConfig::binary();
                if let Some(always_ram) = parse_optional_param::<bool>(params, "always_ram")? {
                    cfg = cfg.with_always_ram(always_ram);
                }
                config = config.with_quantization(cfg);
            }
            "product" | "pq" => {
                let compression_raw = params
                    .get("compression")
                    .map(|s| s.to_lowercase())
                    .unwrap_or_else(|| "x4".to_string());
                let ratio = match compression_raw.as_str() {
                    "x4" | "4" => CompressionRatio::X4,
                    "x8" | "8" => CompressionRatio::X8,
                    "x16" | "16" => CompressionRatio::X16,
                    "x32" | "32" => CompressionRatio::X32,
                    "x64" | "64" => CompressionRatio::X64,
                    other => {
                        return Err(Status::invalid_argument(format!(
                            "unknown compression '{}', expected x4/x8/x16/x32/x64",
                            other
                        )))
                    }
                };
                let mut cfg = vector_search::QuantizationConfig::product(ratio);
                if let Some(always_ram) = parse_optional_param::<bool>(params, "always_ram")? {
                    cfg = cfg.with_always_ram(always_ram);
                }
                config = config.with_quantization(cfg);
            }
            other => {
                return Err(Status::invalid_argument(format!(
                    "unknown quantization '{}', expected scalar, binary, product or none",
                    other
                )))
            }
        }
    }
    Ok(config)
}

/// Render a collection config's index tier with its proto spelling.
#[cfg(feature = "vector")]
fn grpc_index_type_name(config: &vector_search::CollectionConfig) -> String {
    use vector_search::IndexType;
    match config.index_type {
        Some(IndexType::FLAT) => "FLAT".to_string(),
        Some(IndexType::IVF) => "IVF".to_string(),
        Some(IndexType::HNSW) | None => "HNSW".to_string(),
    }
}

/// Build a proto index descriptor from stored collection metadata.
#[cfg(feature = "vector")]
fn grpc_index_info_to_proto(
    space_name: String,
    tag_name: String,
    field_name: String,
    meta: &vector_search::IndexMetadata,
) -> super::proto::VectorIndexInfo {
    super::proto::VectorIndexInfo {
        space_name,
        tag_name,
        field_name,
        options: Some(super::proto::VectorIndexOptions {
            dimension: meta.config.vector_size as i32,
            metric: distance_to_proto_metric(meta.config.distance),
            index_type: grpc_index_type_name(&meta.config),
            parameters: std::collections::HashMap::new(),
        }),
        created_at: meta.created_at.timestamp(),
        indexed_vectors: meta.vector_count as i64,
    }
}

/// Convert a core [`Value`] to a protobuf [`super::proto::Value`].
fn value_to_proto_value(value: graphdb_core::Value) -> super::proto::Value {
    use super::proto::value::Value as ProtoValue;
    use super::proto::Value as ProtoValueMsg;

    let proto_val = match value {
        graphdb_core::Value::Empty | graphdb_core::Value::Null(_) => {
            ProtoValue::StringValue(String::new())
        }
        graphdb_core::Value::Bool(b) => ProtoValue::BoolValue(b),
        graphdb_core::Value::SmallInt(i) => ProtoValue::IntValue(i as i64),
        graphdb_core::Value::Int(i) => ProtoValue::IntValue(i as i64),
        graphdb_core::Value::BigInt(i) => ProtoValue::IntValue(i),
        graphdb_core::Value::Float(f) => ProtoValue::FloatValue(f as f64),
        graphdb_core::Value::Double(d) => ProtoValue::DoubleValue(d),
        graphdb_core::Value::Decimal128(d) => ProtoValue::StringValue(d.to_string()),
        graphdb_core::Value::String(s) => ProtoValue::StringValue(s.to_string()),
        graphdb_core::Value::FixedString(data) => ProtoValue::StringValue(data),
        graphdb_core::Value::Date(d) => ProtoValue::StringValue(d.to_string()),
        graphdb_core::Value::Time(t) => ProtoValue::StringValue(t.to_string()),
        graphdb_core::Value::DateTime(dt) => ProtoValue::StringValue(dt.to_string()),
        graphdb_core::Value::Blob(b) => ProtoValue::BytesValue(b),
        other => ProtoValue::StringValue(format!("{:?}", other)),
    };

    ProtoValueMsg {
        value: Some(proto_val),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_service_creation() {
        // Test that the service can be created
        // Note: This is a placeholder test
        // Actual tests would require mocking AppState and Config
    }
}
