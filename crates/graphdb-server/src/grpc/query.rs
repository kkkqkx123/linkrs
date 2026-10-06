//! Query execution handlers including streaming.

use std::collections::HashMap;

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::convert::{proto_value_to_core, value_to_proto_value};
use super::error::parse_session_id;
use super::proto::*;
use super::service::{ExecuteQueryStreamStream, GraphDBService};

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
    pub(crate) async fn handle_execute_query(
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
            Err(e) if e.message().contains("Invalid session ID") => {
                Err(Status::unauthenticated(e.to_string()))
            }
            Err(e) => Ok(Response::new(ExecuteQueryResponse {
                success: false,
                result: None,
                error: e.to_string(),
                metadata: None,
            })),
        }
    }

    pub(crate) async fn handle_validate_query(
        &self,
        request: Request<ValidateQueryRequest>,
    ) -> Result<Response<ValidateQueryResponse>, Status> {
        let inner = request.into_inner();
        let session_id = inner.session_id.parse::<i64>().unwrap_or(0);
        match crate::http::handlers::query::validate_gql(&inner.query) {
            Ok(parameter_names) => {
                let (estimated_rows, has_estimate) = if !inner.need_estimate
                    || crate::graph_service::GraphService::<S>::is_command_like(&inner.query)
                {
                    (0, false)
                } else {
                    match self
                        .app_state
                        .server
                        .get_graph_service()
                        .estimate_rows(session_id, &inner.query)
                        .await
                    {
                        Some(rows) => (rows, true),
                        None => (0, false),
                    }
                };
                Ok(Response::new(ValidateQueryResponse {
                    valid: true,
                    error: String::new(),
                    parameter_names,
                    estimated_rows,
                    has_estimate,
                }))
            }
            Err(e) => Ok(Response::new(ValidateQueryResponse {
                valid: false,
                error: e,
                parameter_names: vec![],
                estimated_rows: 0,
                has_estimate: false,
            })),
        }
    }

    pub(crate) async fn handle_execute_query_stream(
        &self,
        request: Request<ExecuteQueryRequest>,
    ) -> Result<Response<ExecuteQueryStreamStream>, Status> {
        let inner = request.into_inner();
        let session_id: i64 = inner.session_id.unwrap_or_default().parse().unwrap_or(0);
        let query = inner.query;

        let graph_service = self.app_state.server.get_graph_service();

        let stream_result = graph_service
            .execute_stream(session_id, &query)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamResponse, Status>>(16);

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
                        as ExecuteQueryStreamStream,
                ));
            }
            schema_sent.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        let schema_sent_clone = schema_sent.clone();

        tokio::spawn(async move {
            let tx_pull = tx.clone();

            let pull_handle = tokio::task::spawn_blocking(move || loop {
                match stream_result.next_chunk() {
                    Ok(Some(chunk)) => {
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
                            stream_result.cancel();
                            return;
                        }
                    }
                    Ok(None) => {
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
            });

            let _ = pull_handle.await;
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as ExecuteQueryStreamStream))
    }
}

/// Render a core `QueryResult` into the proto result/metadata pair.
///
/// Rows come from the engine `ExecutionResult` unchanged (column order
/// preserved); non-dataset results yield empty columns and rows.
pub(crate) fn query_result_to_proto(
    result: &graphdb_api::api_core::QueryResult,
) -> (
    Option<super::proto::QueryResult>,
    Option<super::proto::ExecutionMetadata>,
) {
    use std::collections::HashMap;
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
