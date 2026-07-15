//! gRPC Server Implementation
//!
//! Provides a gRPC-based interface to GraphDB services.

use std::net::SocketAddr;
use std::time::Instant;
use tonic::{transport::Server, Request, Response, Status};

use crate::api::server::http::AppState;
use crate::config::Config;

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
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
        _request: Request<LoginRequest>,
    ) -> Result<Response<LoginResponse>, Status> {
        // TODO: Implement authentication logic
        // This should integrate with the existing auth service

        Ok(Response::new(LoginResponse {
            success: true,
            session_id: "session_id".to_string(),
            error: String::new(),
        }))
    }

    async fn logout(
        &self,
        _request: Request<LogoutRequest>,
    ) -> Result<Response<LogoutResponse>, Status> {
        // TODO: Implement logout logic

        Ok(Response::new(LogoutResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn create_session(
        &self,
        _request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        // TODO: Implement session creation logic

        Ok(Response::new(CreateSessionResponse {
            success: true,
            session_id: "session_id".to_string(),
            space_id: 0,
            error: String::new(),
        }))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let session_id = request.into_inner().session_id;

        // TODO: Implement session retrieval logic

        Ok(Response::new(GetSessionResponse {
            exists: true,
            session_id,
            username: "user".to_string(),
            space_id: 0,
            created_at: 0,
            last_accessed: 0,
        }))
    }

    async fn close_session(
        &self,
        _request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        // TODO: Implement session close logic

        Ok(Response::new(CloseSessionResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn execute_query(
        &self,
        _request: Request<ExecuteQueryRequest>,
    ) -> Result<Response<ExecuteQueryResponse>, Status> {
        // TODO: Implement query execution logic
        // This should integrate with the existing QueryApi

        Ok(Response::new(ExecuteQueryResponse {
            success: true,
            result: None,
            error: String::new(),
            metadata: None,
        }))
    }

    async fn validate_query(
        &self,
        _request: Request<ValidateQueryRequest>,
    ) -> Result<Response<ValidateQueryResponse>, Status> {
        // TODO: Implement query validation logic

        Ok(Response::new(ValidateQueryResponse {
            valid: true,
            error: String::new(),
            parameter_names: vec![],
        }))
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
        _request: Request<BeginTransactionRequest>,
    ) -> Result<Response<BeginTransactionResponse>, Status> {
        // TODO: Implement transaction begin logic

        Ok(Response::new(BeginTransactionResponse {
            success: true,
            transaction_id: "txn_id".to_string(),
            error: String::new(),
        }))
    }

    async fn commit_transaction(
        &self,
        _request: Request<CommitTransactionRequest>,
    ) -> Result<Response<CommitTransactionResponse>, Status> {
        // TODO: Implement transaction commit logic

        Ok(Response::new(CommitTransactionResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn rollback_transaction(
        &self,
        _request: Request<RollbackTransactionRequest>,
    ) -> Result<Response<RollbackTransactionResponse>, Status> {
        // TODO: Implement transaction rollback logic

        Ok(Response::new(RollbackTransactionResponse {
            success: true,
            error: String::new(),
        }))
    }

    // Schema Management - Space
    async fn create_space(
        &self,
        _request: Request<CreateSpaceRequest>,
    ) -> Result<Response<CreateSpaceResponse>, Status> {
        unimplemented!("CreateSpace not yet implemented")
    }

    async fn get_space(
        &self,
        _request: Request<GetSpaceRequest>,
    ) -> Result<Response<GetSpaceResponse>, Status> {
        unimplemented!("GetSpace not yet implemented")
    }

    async fn drop_space(
        &self,
        _request: Request<DropSpaceRequest>,
    ) -> Result<Response<DropSpaceResponse>, Status> {
        unimplemented!("DropSpace not yet implemented")
    }

    async fn list_spaces(
        &self,
        _request: Request<ListSpacesRequest>,
    ) -> Result<Response<ListSpacesResponse>, Status> {
        unimplemented!("ListSpaces not yet implemented")
    }

    // Schema Management - Tag
    async fn create_tag(
        &self,
        _request: Request<CreateTagRequest>,
    ) -> Result<Response<CreateTagResponse>, Status> {
        unimplemented!("CreateTag not yet implemented")
    }

    async fn get_tag(
        &self,
        _request: Request<GetTagRequest>,
    ) -> Result<Response<GetTagResponse>, Status> {
        unimplemented!("GetTag not yet implemented")
    }

    async fn list_tags(
        &self,
        _request: Request<ListTagsRequest>,
    ) -> Result<Response<ListTagsResponse>, Status> {
        unimplemented!("ListTags not yet implemented")
    }

    async fn drop_tag(
        &self,
        _request: Request<DropTagRequest>,
    ) -> Result<Response<DropTagResponse>, Status> {
        unimplemented!("DropTag not yet implemented")
    }

    // Schema Management - Edge Type
    async fn create_edge_type(
        &self,
        _request: Request<CreateEdgeTypeRequest>,
    ) -> Result<Response<CreateEdgeTypeResponse>, Status> {
        unimplemented!("CreateEdgeType not yet implemented")
    }

    async fn get_edge_type(
        &self,
        _request: Request<GetEdgeTypeRequest>,
    ) -> Result<Response<GetEdgeTypeResponse>, Status> {
        unimplemented!("GetEdgeType not yet implemented")
    }

    async fn list_edge_types(
        &self,
        _request: Request<ListEdgeTypesRequest>,
    ) -> Result<Response<ListEdgeTypesResponse>, Status> {
        unimplemented!("ListEdgeTypes not yet implemented")
    }

    async fn drop_edge_type(
        &self,
        _request: Request<DropEdgeTypeRequest>,
    ) -> Result<Response<DropEdgeTypeResponse>, Status> {
        unimplemented!("DropEdgeType not yet implemented")
    }

    // Batch Operations
    async fn create_batch(
        &self,
        _request: Request<CreateBatchRequest>,
    ) -> Result<Response<CreateBatchResponse>, Status> {
        unimplemented!("CreateBatch not yet implemented")
    }

    async fn add_batch_items(
        &self,
        _request: Request<AddBatchItemsRequest>,
    ) -> Result<Response<AddBatchItemsResponse>, Status> {
        unimplemented!("AddBatchItems not yet implemented")
    }

    async fn execute_batch(
        &self,
        _request: Request<ExecuteBatchRequest>,
    ) -> Result<Response<ExecuteBatchResponse>, Status> {
        unimplemented!("ExecuteBatch not yet implemented")
    }

    async fn get_batch_status(
        &self,
        _request: Request<GetBatchStatusRequest>,
    ) -> Result<Response<GetBatchStatusResponse>, Status> {
        unimplemented!("GetBatchStatus not yet implemented")
    }

    async fn cancel_batch(
        &self,
        _request: Request<CancelBatchRequest>,
    ) -> Result<Response<CancelBatchResponse>, Status> {
        unimplemented!("CancelBatch not yet implemented")
    }

    // Statistics
    async fn get_session_statistics(
        &self,
        _request: Request<GetSessionStatisticsRequest>,
    ) -> Result<Response<GetSessionStatisticsResponse>, Status> {
        unimplemented!("GetSessionStatistics not yet implemented")
    }

    async fn get_query_statistics(
        &self,
        _request: Request<GetQueryStatisticsRequest>,
    ) -> Result<Response<GetQueryStatisticsResponse>, Status> {
        unimplemented!("GetQueryStatistics not yet implemented")
    }

    async fn get_database_statistics(
        &self,
        _request: Request<GetDatabaseStatisticsRequest>,
    ) -> Result<Response<GetDatabaseStatisticsResponse>, Status> {
        unimplemented!("GetDatabaseStatistics not yet implemented")
    }

    async fn get_system_statistics(
        &self,
        _request: Request<GetSystemStatisticsRequest>,
    ) -> Result<Response<GetSystemStatisticsResponse>, Status> {
        unimplemented!("GetSystemStatistics not yet implemented")
    }

    // Configuration
    async fn get_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        unimplemented!("GetConfig not yet implemented")
    }

    async fn update_config(
        &self,
        _request: Request<UpdateConfigRequest>,
    ) -> Result<Response<UpdateConfigResponse>, Status> {
        unimplemented!("UpdateConfig not yet implemented")
    }

    async fn reset_config(
        &self,
        _request: Request<ResetConfigRequest>,
    ) -> Result<Response<ResetConfigResponse>, Status> {
        unimplemented!("ResetConfig not yet implemented")
    }

    // Custom Functions
    async fn register_function(
        &self,
        _request: Request<RegisterFunctionRequest>,
    ) -> Result<Response<RegisterFunctionResponse>, Status> {
        unimplemented!("RegisterFunction not yet implemented")
    }

    async fn unregister_function(
        &self,
        _request: Request<UnregisterFunctionRequest>,
    ) -> Result<Response<UnregisterFunctionResponse>, Status> {
        unimplemented!("UnregisterFunction not yet implemented")
    }

    async fn list_functions(
        &self,
        _request: Request<ListFunctionsRequest>,
    ) -> Result<Response<ListFunctionsResponse>, Status> {
        unimplemented!("ListFunctions not yet implemented")
    }

    async fn get_function_info(
        &self,
        _request: Request<GetFunctionInfoRequest>,
    ) -> Result<Response<GetFunctionInfoResponse>, Status> {
        unimplemented!("GetFunctionInfo not yet implemented")
    }

    // Vector Index
    async fn create_vector_index(
        &self,
        _request: Request<CreateVectorIndexRequest>,
    ) -> Result<Response<CreateVectorIndexResponse>, Status> {
        unimplemented!("CreateVectorIndex not yet implemented")
    }

    async fn get_vector_index(
        &self,
        _request: Request<GetVectorIndexRequest>,
    ) -> Result<Response<GetVectorIndexResponse>, Status> {
        unimplemented!("GetVectorIndex not yet implemented")
    }

    async fn list_vector_indexes(
        &self,
        _request: Request<ListVectorIndexesRequest>,
    ) -> Result<Response<ListVectorIndexesResponse>, Status> {
        unimplemented!("ListVectorIndexes not yet implemented")
    }

    async fn drop_vector_index(
        &self,
        _request: Request<DropVectorIndexRequest>,
    ) -> Result<Response<DropVectorIndexResponse>, Status> {
        unimplemented!("DropVectorIndex not yet implemented")
    }

    async fn search_vector(
        &self,
        _request: Request<SearchVectorRequest>,
    ) -> Result<Response<SearchVectorResponse>, Status> {
        unimplemented!("SearchVector not yet implemented")
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
                        let changes = h
                            .change_log
                            .get_version_changes(version)
                            .cloned()
                            .unwrap_or_default()
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
                            timestamp_ms: 0, // TODO: extract from PropertyChange
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
        + Clone
        + Send
        + Sync
        + 'static,
>(
    app_state: AppState<S>,
    config: Config,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = GraphDBService::new(app_state, config);

    tracing::info!("GraphDB gRPC service listening on {}", addr);

    Server::builder()
        .add_service(GraphDbServiceServer::new(service))
        .serve(addr)
        .await?;

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

/// Convert a core [`Value`] to a protobuf [`super::proto::Value`].
fn value_to_proto_value(value: crate::core::Value) -> super::proto::Value {
    use super::proto::value::Value as ProtoValue;
    use super::proto::Value as ProtoValueMsg;

    let proto_val = match value {
        crate::core::Value::Empty | crate::core::Value::Null(_) => {
            ProtoValue::StringValue(String::new())
        }
        crate::core::Value::Bool(b) => ProtoValue::BoolValue(b),
        crate::core::Value::SmallInt(i) => ProtoValue::IntValue(i as i64),
        crate::core::Value::Int(i) => ProtoValue::IntValue(i as i64),
        crate::core::Value::BigInt(i) => ProtoValue::IntValue(i),
        crate::core::Value::Float(f) => ProtoValue::FloatValue(f as f64),
        crate::core::Value::Double(d) => ProtoValue::DoubleValue(d),
        crate::core::Value::Decimal128(d) => ProtoValue::StringValue(d.to_string()),
        crate::core::Value::String(s) => ProtoValue::StringValue(s),
        crate::core::Value::FixedString { data, .. } => ProtoValue::StringValue(data),
        crate::core::Value::Date(d) => ProtoValue::StringValue(d.to_string()),
        crate::core::Value::Time(t) => ProtoValue::StringValue(t.to_string()),
        crate::core::Value::DateTime(dt) => ProtoValue::StringValue(dt.to_string()),
        crate::core::Value::Blob(b) => ProtoValue::BytesValue(b),
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
