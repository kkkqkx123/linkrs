//! Vector index handlers and option mapping.
//!
//! Transport mapping over the shared `VectorApi`: space names resolve to
//! ids through storage, indexes stay vertex-only like the HTTP path, and
//! option parsing mirrors the HTTP handler so both transports accept the
//! same parameters. Text queries are not supported here; the proto carries
//! an explicit vector only.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[cfg(feature = "vector")]
use super::convert::value_to_proto_value;
use super::proto::*;
use super::service::GraphDBService;

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
    pub(crate) async fn handle_create_vector_index(
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

    pub(crate) async fn handle_get_vector_index(
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

    pub(crate) async fn handle_list_vector_indexes(
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

    pub(crate) async fn handle_drop_vector_index(
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

    pub(crate) async fn handle_search_vector(
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

    pub(crate) async fn handle_scroll_vector(
        &self,
        request: Request<ScrollVectorRequest>,
    ) -> Result<Response<ScrollVectorResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            if req.tag_name.is_empty() || req.field_name.is_empty() {
                return Err(Status::invalid_argument(
                    "tag_name and field_name are required",
                ));
            }
            let wire = crate::http::handlers::vector::ScrollRequest {
                space_id: req.space_id,
                tag_name: req.tag_name,
                field_name: req.field_name,
                limit: req.limit.unwrap_or(100) as usize,
                offset: req.offset.filter(|s| !s.is_empty()),
                with_payload: None,
                with_vector: None,
            };
            match crate::http::handlers::vector::scroll(
                axum::extract::State(self.app_state.clone()),
                axum::Json(wire),
            )
            .await
            {
                Ok(axum::Json(resp)) => Ok(Response::new(ScrollVectorResponse {
                    points_json: serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string()),
                    error: String::new(),
                })),
                Err(e) => Err(vector_http_status(e)),
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    pub(crate) async fn handle_get_vector_count(
        &self,
        request: Request<GetVectorCountRequest>,
    ) -> Result<Response<GetVectorCountResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            match crate::http::handlers::vector::count(
                axum::extract::State(self.app_state.clone()),
                axum::extract::Path((req.space_id, req.tag_name, req.field_name)),
            )
            .await
            {
                Ok(axum::Json(resp)) => {
                    let count = resp.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                    Ok(Response::new(GetVectorCountResponse {
                        count,
                        error: String::new(),
                    }))
                }
                Err(e) => Err(vector_http_status(e)),
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    pub(crate) async fn handle_rebuild_vector_index(
        &self,
        request: Request<RebuildVectorIndexRequest>,
    ) -> Result<Response<RebuildVectorIndexResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            let wire = graphdb_wire::vector::RebuildVectorIndexRequest {
                space_id: req.space_id,
                tag_name: req.tag_name,
                field_name: req.field_name,
            };
            match crate::http::handlers::rebuild::rebuild_vector(
                axum::extract::State(self.app_state.clone()),
                axum::Json(wire),
            )
            .await
            {
                Ok(axum::Json(resp)) => Ok(Response::new(RebuildVectorIndexResponse {
                    rebuild_id: resp.rebuild_id,
                    status: resp.status,
                    error: String::new(),
                })),
                Err(e) => Err(vector_http_status(e)),
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    pub(crate) async fn handle_get_vector_rebuild_status(
        &self,
        request: Request<GetVectorRebuildStatusRequest>,
    ) -> Result<Response<GetVectorRebuildStatusResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            match crate::http::handlers::rebuild::vector_rebuild_status(
                axum::extract::State(self.app_state.clone()),
                axum::extract::Path(req.rebuild_id.clone()),
            )
            .await
            {
                Ok(axum::Json(resp)) => {
                    let progress_json =
                        serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
                    Ok(Response::new(GetVectorRebuildStatusResponse {
                        rebuild_id: resp.rebuild_id,
                        status: resp.status,
                        progress_json,
                        error: String::new(),
                    }))
                }
                Err(e) => Err(vector_http_status(e)),
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    pub(crate) async fn handle_clear_vector_index(
        &self,
        request: Request<ClearVectorIndexRequest>,
    ) -> Result<Response<ClearVectorIndexResponse>, Status> {
        #[cfg(feature = "vector")]
        {
            let req = request.into_inner();
            if !req.force {
                return Err(Status::invalid_argument(
                    "Clearing a vector index is destructive and requires force=true",
                ));
            }
            let wire = graphdb_wire::vector::ClearVectorIndexRequest {
                space_id: req.space_id,
                tag_name: req.tag_name,
                field_name: req.field_name,
                force: true,
            };
            match crate::http::handlers::rebuild::clear_vector(
                axum::extract::State(self.app_state.clone()),
                axum::Json(wire),
            )
            .await
            {
                Ok(axum::Json(resp)) => Ok(Response::new(ClearVectorIndexResponse {
                    ok: resp.ok,
                    error: String::new(),
                })),
                Err(e) => Err(vector_http_status(e)),
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    pub(crate) async fn handle_get_vector_point(
        &self,
        request: Request<GetVectorPointRequest>,
    ) -> Result<Response<GetVectorPointResponse>, Status> {
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
            match vector_api
                .get_vector(req.space_id, &req.tag_name, &req.field_name, &req.point_id)
                .await
            {
                Ok(Some(point)) => Ok(Response::new(GetVectorPointResponse {
                    exists: true,
                    point_json: serde_json::to_string(&serde_json::json!({
                        "id": point.id.to_string(),
                        "vector": point.vector,
                        "payload": point.payload,
                    }))
                    .unwrap_or_else(|_| "{}".to_string()),
                    error: String::new(),
                })),
                Ok(None) => Ok(Response::new(GetVectorPointResponse {
                    exists: false,
                    point_json: String::new(),
                    error: String::new(),
                })),
                Err(e) => Err(Status::internal(e.to_string())),
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    pub(crate) async fn handle_set_vector_payload(
        &self,
        request: Request<SetVectorPayloadRequest>,
    ) -> Result<Response<SetVectorPayloadResponse>, Status> {
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
            let payload: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&req.payload_json)
                    .map_err(|e| Status::invalid_argument(format!("invalid payload_json: {}", e)))?;
            let point_ids: Vec<&str> =
                req.point_ids.iter().map(|s| s.as_str()).collect();
            vector_api
                .set_payload(
                    req.space_id,
                    &req.tag_name,
                    &req.field_name,
                    point_ids,
                    payload.into_iter().collect(),
                )
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            Ok(Response::new(SetVectorPayloadResponse {
                success: true,
                error: String::new(),
            }))
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    pub(crate) async fn handle_set_vector_payload_fields(
        &self,
        request: Request<SetVectorPayloadFieldsRequest>,
    ) -> Result<Response<SetVectorPayloadFieldsResponse>, Status> {
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
            let payload: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&req.payload_json)
                    .map_err(|e| Status::invalid_argument(format!("invalid payload_json: {}", e)))?;
            let point_ids: Vec<&str> =
                req.point_ids.iter().map(|s| s.as_str()).collect();
            vector_api
                .set_payload_fields(
                    req.space_id,
                    &req.tag_name,
                    &req.field_name,
                    point_ids,
                    payload.into_iter().collect(),
                )
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            Ok(Response::new(SetVectorPayloadFieldsResponse {
                success: true,
                error: String::new(),
            }))
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }

    pub(crate) async fn handle_delete_vector_payload(
        &self,
        request: Request<DeleteVectorPayloadRequest>,
    ) -> Result<Response<DeleteVectorPayloadResponse>, Status> {
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
            let point_ids: Vec<&str> =
                req.point_ids.iter().map(|s| s.as_str()).collect();
            let keys: Vec<&str> = req.keys.iter().map(|s| s.as_str()).collect();
            vector_api
                .delete_payload(
                    req.space_id,
                    &req.tag_name,
                    &req.field_name,
                    point_ids,
                    keys,
                )
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            Ok(Response::new(DeleteVectorPayloadResponse {
                success: true,
                error: String::new(),
            }))
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = request;
            Err(Status::unavailable("vector support is not compiled in"))
        }
    }
}

#[allow(dead_code)]
fn vector_http_status(error: crate::http::error::HttpError) -> Status {
    use crate::http::error::HttpError;
    match error {
        HttpError::BadRequest(message) => Status::invalid_argument(message),
        HttpError::NotFound(message) => Status::not_found(message),
        HttpError::Conflict(message) => Status::already_exists(message),
        HttpError::Unauthorized(message) => Status::unauthenticated(message),
        HttpError::InternalError(message) => Status::internal(message),
    }
}

/// Map a proto `DistanceMetric` discriminant to the local metric.
///
/// Only Cosine, L2 and Dot exist on the wire; anything else is rejected so a
/// caller can never silently create an index with a different metric.
#[cfg(feature = "vector")]
pub(crate) fn proto_metric_to_distance(
    metric: i32,
) -> Result<vector_search::DistanceMetric, Status> {
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
pub(crate) fn distance_to_proto_metric(metric: vector_search::DistanceMetric) -> i32 {
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
pub(crate) fn parse_optional_param<T>(
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
pub(crate) fn grpc_collection_config(
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
pub(crate) fn grpc_index_type_name(config: &vector_search::CollectionConfig) -> String {
    use vector_search::IndexType;
    match config.index_type {
        Some(IndexType::FLAT) => "FLAT".to_string(),
        Some(IndexType::IVF) => "IVF".to_string(),
        Some(IndexType::HNSW) | None => "HNSW".to_string(),
    }
}

/// Build a proto index descriptor from stored collection metadata.
#[cfg(feature = "vector")]
pub(crate) fn grpc_index_info_to_proto(
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
