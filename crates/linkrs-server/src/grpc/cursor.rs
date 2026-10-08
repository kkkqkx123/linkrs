//! Forward-only cursor RPCs (parity with `/v1/query/cursor/*`).

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::error::parse_session_id;
use super::proto::*;
use super::service::GraphDBService;

fn require_session(value: &Option<String>) -> Result<i64, Status> {
    match value.as_deref().filter(|s| !s.is_empty()) {
        Some(raw) => parse_session_id(raw),
        None => Err(Status::unauthenticated("session_id is required")),
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
    pub(crate) async fn handle_open_cursor(
        &self,
        request: Request<OpenCursorRequest>,
    ) -> Result<Response<OpenCursorResponse>, Status> {
        let req = request.into_inner();
        if req.query.trim().is_empty() {
            return Err(Status::invalid_argument("query must not be empty"));
        }
        let session_id = require_session(&req.session_id)?;
        let graph_service = self.app_state.server.get_graph_service();
        match graph_service.open_cursor(session_id, &req.query).await {
            Ok((cursor_id, columns)) => Ok(Response::new(OpenCursorResponse {
                success: true,
                cursor_id,
                columns,
                error: String::new(),
            })),
            Err(message) => Ok(Response::new(OpenCursorResponse {
                success: false,
                cursor_id: 0,
                columns: Vec::new(),
                error: message,
            })),
        }
    }

    pub(crate) async fn handle_fetch_cursor(
        &self,
        request: Request<FetchCursorRequest>,
    ) -> Result<Response<FetchCursorResponse>, Status> {
        let req = request.into_inner();
        let session_id = require_session(&req.session_id)?;
        let page_size = usize::try_from(req.page_size).unwrap_or(500).max(1);
        let graph_service = self.app_state.server.get_graph_service();
        match graph_service
            .fetch_cursor(session_id, req.cursor_id, page_size)
            .await
        {
            Ok(page) => {
                let rows: Vec<std::collections::HashMap<String, serde_json::Value>> = page
                    .rows
                    .into_iter()
                    .map(|row| {
                        page.columns
                            .iter()
                            .zip(row)
                            .map(|(col, value)| (col.clone(), crate::value::to_json(value)))
                            .collect()
                    })
                    .collect();
                let returned = rows.len() as u64;
                let rows_json = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string());
                Ok(Response::new(FetchCursorResponse {
                    success: true,
                    columns: page.columns,
                    rows_json,
                    has_more: page.has_more,
                    returned,
                    error: String::new(),
                }))
            }
            Err(message) => Ok(Response::new(FetchCursorResponse {
                success: false,
                columns: Vec::new(),
                rows_json: "[]".to_string(),
                has_more: false,
                returned: 0,
                error: message,
            })),
        }
    }

    pub(crate) async fn handle_close_cursor(
        &self,
        request: Request<CloseCursorRequest>,
    ) -> Result<Response<CloseCursorResponse>, Status> {
        let req = request.into_inner();
        let session_id = require_session(&req.session_id)?;
        let graph_service = self.app_state.server.get_graph_service();
        match graph_service.close_cursor(session_id, req.cursor_id).await {
            Ok(closed) => Ok(Response::new(CloseCursorResponse {
                success: true,
                closed,
                error: String::new(),
            })),
            Err(message) => Ok(Response::new(CloseCursorResponse {
                success: false,
                closed: false,
                error: message,
            })),
        }
    }
}
