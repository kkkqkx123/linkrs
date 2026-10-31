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
use super::service::LinkrsService;

#[allow(dead_code)]
fn http_status(error: HttpError) -> Status {
    match error {
        HttpError::BadRequest(message) => Status::invalid_argument(message),
        HttpError::NotFound(message) => Status::not_found(message),
        HttpError::Conflict(message) => Status::already_exists(message),
        HttpError::Unauthorized(message) => Status::unauthenticated(message),
        HttpError::Forbidden(message) => Status::permission_denied(message),
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
    > LinkrsService<S>
{
    pub(crate) async fn handle_rebuild_fulltext_index(
        &self,
        request: Request<RebuildFulltextIndexRequest>,
    ) -> Result<Response<RebuildFulltextIndexResponse>, Status> {
        #[cfg(feature = "fulltext")]
        {
            let req = request.into_inner();
            let session_id = super::error::require_session_id(&req.session_id)?;
            let wire = linkrs_wire::fulltext::RebuildFulltextIndexRequest {
                space_id: req.space_id,
                tag_name: req.tag_name,
                field_name: req.field_name,
            };
            match crate::services::rebuild::rebuild_fulltext_core(&self.app_state, session_id, wire)
                .await
            {
                Ok(resp) => Ok(Response::new(RebuildFulltextIndexResponse {
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
            let session_id = super::error::require_session_id(&req.session_id)?;
            match crate::services::rebuild::fulltext_rebuild_status_core(
                &self.app_state,
                session_id,
                req.rebuild_id.clone(),
            )
            .await
            {
                Ok(resp) => {
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
            let session_id = super::error::require_session_id(&req.session_id)?;
            if !req.force {
                return Err(Status::invalid_argument(
                    "Clearing a fulltext index is destructive and requires force=true",
                ));
            }
            let wire = linkrs_wire::fulltext::ClearFulltextIndexRequest {
                space_id: req.space_id,
                tag_name: req.tag_name,
                field_name: req.field_name,
                force: true,
            };
            match crate::services::rebuild::clear_fulltext_core(&self.app_state, session_id, wire)
                .await
            {
                Ok(resp) => Ok(Response::new(ClearFulltextIndexResponse {
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
            let req = request.into_inner();
            let session_id = super::error::require_session_id(&req.session_id)?;
            match crate::services::rebuild::inconsistent_fulltext_core(&self.app_state, session_id)
                .await
            {
                Ok(resp) => Ok(Response::new(ListInconsistentFulltextResponse {
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

    pub(crate) async fn handle_search_fulltext(
        &self,
        request: Request<SearchFulltextRequest>,
    ) -> Result<Response<SearchFulltextResponse>, Status> {
        #[cfg(feature = "fulltext")]
        {
            use crate::services::fulltext::FulltextSearchRequestBody;
            let req = request.into_inner();
            let session_id = super::error::require_session_id(&req.session_id)?;
            let limit = if req.limit == 0 {
                10
            } else {
                req.limit as usize
            };
            let body = FulltextSearchRequestBody {
                space_id: req.space_id,
                tag_name: req.tag_name,
                field_name: req.field_name,
                query: req.query,
                limit,
            };
            match crate::services::fulltext::search_core(&self.app_state, session_id, body).await {
                Ok(resp) => {
                    let hits = resp
                        .results
                        .into_iter()
                        .map(|h| super::proto::FulltextSearchHit {
                            doc_id: h.doc_id,
                            score: h.score,
                            highlights: h.highlights.unwrap_or_default(),
                        })
                        .collect();
                    Ok(Response::new(SearchFulltextResponse {
                        results: hits,
                        total_hits: resp.total_hits as u64,
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
}
