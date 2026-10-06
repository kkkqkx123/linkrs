//! Fulltext index RPCs (parity with `/v1/fulltext/*`).
//!
//! Thin wrappers over the HTTP rebuild handlers so both transports share
//! one rebuild driver, task registry and progress view.

use tonic::{Request, Response, Status};

use crate::http::error::HttpError;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::proto::*;
use super::service::GraphDBService;

#[allow(dead_code)]
fn http_status(error: HttpError) -> Status {
    match error {
        HttpError::BadRequest(message) => Status::invalid_argument(message),
        HttpError::NotFound(message) => Status::not_found(message),
        HttpError::Conflict(message) => Status::already_exists(message),
        HttpError::Unauthorized(message) => Status::unauthenticated(message),
        HttpError::InternalError(message) => Status::internal(message),
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
    pub(crate) async fn handle_rebuild_fulltext_index(
        &self,
        request: Request<RebuildFulltextIndexRequest>,
    ) -> Result<Response<RebuildFulltextIndexResponse>, Status> {
        #[cfg(feature = "fulltext")]
        {
            let req = request.into_inner();
            let wire = graphdb_wire::fulltext::RebuildFulltextIndexRequest {
                space_id: req.space_id,
                tag_name: req.tag_name,
                field_name: req.field_name,
            };
            match crate::http::handlers::rebuild::rebuild_fulltext(
                axum::extract::State(self.app_state.clone()),
                axum::Json(wire),
            )
            .await
            {
                Ok(axum::Json(resp)) => Ok(Response::new(RebuildFulltextIndexResponse {
                    rebuild_id: resp.rebuild_id,
                    status: resp.status,
                    error: String::new(),
                })),
                Err(e) => Err(http_status(e)),
            }
        }
        #[cfg(not(feature = "fulltext"))]
        {
            let _ = request;
            Err(Status::unimplemented("fulltext feature is not enabled"))
        }
    }

    pub(crate) async fn handle_get_fulltext_rebuild_status(
        &self,
        request: Request<GetFulltextRebuildStatusRequest>,
    ) -> Result<Response<GetFulltextRebuildStatusResponse>, Status> {
        #[cfg(feature = "fulltext")]
        {
            let req = request.into_inner();
            match crate::http::handlers::rebuild::fulltext_rebuild_status(
                axum::extract::State(self.app_state.clone()),
                axum::extract::Path(req.rebuild_id.clone()),
            )
            .await
            {
                Ok(axum::Json(resp)) => {
                    let progress_json =
                        serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
                    Ok(Response::new(GetFulltextRebuildStatusResponse {
                        rebuild_id: req.rebuild_id,
                        status: resp.status,
                        progress_json,
                        error: String::new(),
                    }))
                }
                Err(e) => Err(http_status(e)),
            }
        }
        #[cfg(not(feature = "fulltext"))]
        {
            let _ = request;
            Err(Status::unimplemented("fulltext feature is not enabled"))
        }
    }

    pub(crate) async fn handle_clear_fulltext_index(
        &self,
        request: Request<ClearFulltextIndexRequest>,
    ) -> Result<Response<ClearFulltextIndexResponse>, Status> {
        #[cfg(feature = "fulltext")]
        {
            let req = request.into_inner();
            if !req.force {
                return Err(Status::invalid_argument(
                    "Clearing a fulltext index is destructive and requires force=true",
                ));
            }
            let wire = graphdb_wire::fulltext::ClearFulltextIndexRequest {
                space_id: req.space_id,
                tag_name: req.tag_name,
                field_name: req.field_name,
                force: true,
            };
            match crate::http::handlers::rebuild::clear_fulltext(
                axum::extract::State(self.app_state.clone()),
                axum::Json(wire),
            )
            .await
            {
                Ok(axum::Json(resp)) => Ok(Response::new(ClearFulltextIndexResponse {
                    ok: resp.ok,
                    error: String::new(),
                })),
                Err(e) => Err(http_status(e)),
            }
        }
        #[cfg(not(feature = "fulltext"))]
        {
            let _ = request;
            Err(Status::unimplemented("fulltext feature is not enabled"))
        }
    }

    pub(crate) async fn handle_list_inconsistent_fulltext(
        &self,
        request: Request<ListInconsistentFulltextRequest>,
    ) -> Result<Response<ListInconsistentFulltextResponse>, Status> {
        #[cfg(feature = "fulltext")]
        {
            let _ = request;
            match crate::http::handlers::rebuild::inconsistent_fulltext(axum::extract::State(
                self.app_state.clone(),
            ))
            .await
            {
                Ok(axum::Json(resp)) => Ok(Response::new(ListInconsistentFulltextResponse {
                    indexes_json: serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string()),
                    error: String::new(),
                })),
                Err(e) => Err(http_status(e)),
            }
        }
        #[cfg(not(feature = "fulltext"))]
        {
            let _ = request;
            Err(Status::unimplemented("fulltext feature is not enabled"))
        }
    }
}
