use axum::{
    extract::{Extension, Json, Path, State},
    response::Json as JsonResponse,
};
use serde::{Deserialize, Serialize};

use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_sync::vector_sync::SearchOptions;
use linkrs_wire::vector::{
    PayloadMutationResponse, SparseVectorData, VectorCountResponse, VectorIndexCreatedResponse,
    VectorIndexDroppedResponse, VectorPoint, VectorPointResponse,
};
use simvec::{DistanceMetric, VectorFilter};

/// Vector index creation request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateVectorIndexRequest {
    pub space_id: u64,
    /// Vertex tag name. Vector indexes are vertex-only; edge type names are
    /// rejected.
    pub tag_name: String,
    pub field_name: String,
    pub vector_size: usize,
    /// Payload layout: `dense` (default) or `sparse`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default = "default_distance")]
    pub distance: DistanceMetric,
    /// HNSW overrides
    pub hnsw_m: Option<usize>,
    pub hnsw_ef_construct: Option<usize>,
    /// Quantization: scalar/binary/product/none (case-insensitive). None = disabled.
    pub quantization: Option<String>,
    /// Scalar only: quantile in (0,1]
    pub quantile: Option<f32>,
    /// Product only: x4/x8/x16/x32/x64 or integer 4/8/16/32/64
    pub compression: Option<String>,
    /// Keep quantized vectors in RAM
    pub always_ram: Option<bool>,
    /// HNSW: live-point threshold above which an exact scan collection is
    /// promoted to a published graph.
    pub hnsw_full_scan_threshold: Option<usize>,
    /// HNSW: build parallelism (`0` = engine default).
    pub hnsw_max_indexing_threads: Option<usize>,
    /// HNSW: keep the graph on disk (remote backends only).
    pub hnsw_on_disk: Option<bool>,
    /// HNSW: filtered-graph connection cap.
    pub hnsw_payload_m: Option<usize>,
    /// Storage layout: shard count (remote backends only).
    pub shard_number: Option<usize>,
    /// Storage layout: replica count (remote backends only).
    pub replication_factor: Option<usize>,
    /// Storage layout: acknowledgements required per write (remote only).
    pub write_consistency_factor: Option<usize>,
    /// Storage layout: keep payloads on disk instead of in RAM.
    pub on_disk_payload: Option<bool>,
    /// Storage layout: keep dense vectors on disk (remote backends only).
    pub vectors_on_disk: Option<bool>,
    /// Admission cap on live points; `0`/absent means unlimited.
    pub max_points: Option<u64>,
}

/// Storage layout of the physical collection backing a logical index.
///
/// Mirrors what the engine reports for the collection rather than what the
/// request asked for, so a caller can tell whether a later request drifted.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VectorStorageInfo {
    pub vectors_on_disk: Option<bool>,
    pub on_disk_payload: Option<bool>,
    pub shard_number: Option<usize>,
    pub replication_factor: Option<usize>,
    pub write_consistency_factor: Option<usize>,
    pub max_points: Option<u64>,
    /// Quantization spelling (`scalar` / `binary` / `product`), `none` when
    /// exact `f32` storage is in force.
    pub quantization: String,
}

impl VectorStorageInfo {
    fn from_config(config: &simvec::CollectionConfig) -> Self {
        let quantization = match config.quantization_config.as_ref() {
            None => "none".to_string(),
            Some(cfg) => match cfg.quant_type {
                simvec::QuantizationType::Scalar { .. } => "scalar".to_string(),
                simvec::QuantizationType::Binary { .. } => "binary".to_string(),
                simvec::QuantizationType::Product { .. } => "product".to_string(),
            },
        };
        Self {
            vectors_on_disk: config.vectors_on_disk,
            on_disk_payload: config.on_disk_payload,
            shard_number: config.shard_number,
            replication_factor: config.replication_factor,
            write_consistency_factor: config.write_consistency_factor,
            max_points: config.max_points,
            quantization,
        }
    }
}

fn default_distance() -> DistanceMetric {
    DistanceMetric::Cosine
}

/// Vector index information
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VectorIndexInfo {
    pub name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub vector_size: usize,
    pub distance: String,
    pub points_count: u64,
    pub status: String,
    /// Storage layout of the backing collection; absent when the engine has
    /// no live collection for this logical index.
    pub storage: Option<VectorStorageInfo>,
}

/// Vector search request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VectorSearchRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    #[serde(default)]
    pub query_vector: Vec<f32>,
    #[serde(default)]
    pub query_data: Option<SparseVectorData>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub threshold: Option<f32>,
    pub filter: Option<VectorFilter>,
    /// Consistency level: "eventual" (default) or "read_your_writes"
    #[serde(default)]
    pub consistency: Option<String>,
    /// Timeout in milliseconds when consistency is read_your_writes
    pub consistency_timeout_ms: Option<u64>,
    /// Minimum LSN to wait for (when RYW)
    pub minimum_lsn: Option<u64>,
}

fn default_limit() -> usize {
    10
}

/// Parse the gRPC `SearchFilter.expression` wire string.
///
/// The wire carries a JSON-encoded `VectorFilter` with the same shape as the
/// HTTP search filter. There is no expression language; arbitrary strings
/// fail with the underlying JSON error.
#[allow(dead_code)]
pub(crate) fn parse_vector_filter_expression(expression: &str) -> Result<VectorFilter, String> {
    serde_json::from_str(expression).map_err(|e| {
        format!("filter.expression must be a JSON VectorFilter with the same shape as the HTTP search filter: {e}")
    })
}

/// Vector search result
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VectorSearchResponse {
    pub results: Vec<VectorSearchResult>,
    pub count: usize,
}

/// Single vector search result
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VectorSearchResult {
    pub doc_id: String,
    pub score: f32,
    pub vector: Option<Vec<f32>>,
    pub data: Option<SparseVectorData>,
    #[schema(value_type = Option<std::collections::HashMap<String, linkrs_wire::query::CellValue>>)]
    pub payload: Option<serde_json::Map<String, serde_json::Value>>,
}

/// List of vector indexes response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListVectorIndexesResponse {
    pub indexes: Vec<VectorIndexInfo>,
    pub count: usize,
}

/// Vector index details response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VectorIndexDetailsResponse {
    pub collection_name: String,
    pub status: String,
    pub vectors_count: u64,
    pub points_count: u64,
    pub indexed_vectors_count: u64,
    pub vector_size: usize,
    pub distance: String,
    pub storage: VectorStorageInfo,
}

#[utoipa::path(
    post,
    operation_id = "post_v1_vector_indexes",
    path = "/v1/vector/indexes",
    tag = "Vector",
    request_body = CreateVectorIndexRequest,
    responses(
        (status = 200, body = VectorIndexCreatedResponse, description = "Vector index created"),
        (status = 500, description = "Internal error")
    )
)]
/// Create a vector index
pub async fn create_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(request): Json<CreateVectorIndexRequest>,
) -> Result<JsonResponse<VectorIndexCreatedResponse>, HttpError> {
    super::authz::require_admin_session(&state, session_id)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        // Vector indexes are vertex-only: point IDs, payloads, and rebuild
        // sources are all modeled per vertex. Reject edge type names up
        // front instead of creating an index that can never receive data.
        {
            let storage = state.server.get_storage();
            let storage = storage.read();
            let space_name = storage
                .get_space_by_id(request.space_id)
                .map_err(|error| HttpError::InternalError(error.to_string()))?
                .map(|info| info.space_name)
                .ok_or_else(|| {
                    HttpError::NotFound(format!("space id {} not found", request.space_id))
                })?;
            let is_edge_type = storage
                .get_edge_type(&space_name, &request.tag_name)
                .map_err(|error| HttpError::InternalError(error.to_string()))?
                .is_some();
            if is_edge_type {
                return Err(HttpError::BadRequest(format!(
                    "Vector indexes are vertex-only: '{}' is an edge type, create the index on a vertex tag instead",
                    request.tag_name
                )));
            }
        }
        // Shared DDL semantics: metric/quantization mapping and validation
        // live in linkrs_query::vector_ddl so this endpoint cannot drift from
        // the CREATE VECTOR INDEX statement pipeline.
        let quantization = match request.quantization.as_deref() {
            Some(raw) => match linkrs_query::vector_ddl::quantization_kind_from_str(raw) {
                Some(parsed) => parsed,
                None => {
                    return Err(HttpError::BadRequest(format!(
                        "unknown quantization '{}', expected scalar/binary/product/none",
                        raw
                    )))
                }
            },
            None => None,
        };
        let compression = match request.compression.as_deref() {
            Some(raw) => match linkrs_query::vector_ddl::compression_from_str(raw) {
                Some(parsed) => Some(parsed),
                None => {
                    return Err(HttpError::BadRequest(format!(
                        "unknown compression '{}', expected x4/x8/x16/x32/x64",
                        raw
                    )))
                }
            },
            None => None,
        };
        let kind = match request.kind.as_deref().unwrap_or("dense") {
            "dense" => Some(linkrs_query::parser::ast::vector::VectorIndexKind::Dense),
            "sparse" => Some(linkrs_query::parser::ast::vector::VectorIndexKind::Sparse),
            other => {
                return Err(HttpError::BadRequest(format!(
                    "unknown vector kind '{other}'"
                )))
            }
        };
        let options = linkrs_query::vector_ddl::CollectionOptions {
            kind,
            hnsw_m: request.hnsw_m,
            hnsw_ef_construct: request.hnsw_ef_construct,
            hnsw_full_scan_threshold: request.hnsw_full_scan_threshold,
            hnsw_max_indexing_threads: request.hnsw_max_indexing_threads,
            hnsw_on_disk: request.hnsw_on_disk,
            hnsw_payload_m: request.hnsw_payload_m,
            quantization,
            quantile: request.quantile,
            compression,
            always_ram: request.always_ram,
            shard_number: request.shard_number,
            replication_factor: request.replication_factor,
            write_consistency_factor: request.write_consistency_factor,
            on_disk_payload: request.on_disk_payload,
            vectors_on_disk: request.vectors_on_disk,
            max_points: request.max_points,
        };
        let config = linkrs_query::vector_ddl::build_collection_config(
            request.vector_size,
            request.distance,
            &options,
        )
        .map_err(HttpError::BadRequest)?;
        let collection_name = vector_api
            .create_index_with_config(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                config,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(VectorIndexCreatedResponse {
            success: true,
            message: "Vector index created successfully".to_string(),
            collection_name,
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    delete,
    path = "/v1/vector/indexes/{space_id}/{tag_name}/{field_name}",
    tag = "Vector",
    params(
        ("space_id" = u64, Path, description = "Space id"),
        ("tag_name" = String, Path, description = "Tag name"),
        ("field_name" = String, Path, description = "Vector field name")
    ),
    responses(
        (status = 200, body = VectorIndexDroppedResponse, description = "Vector index dropped"),
        (status = 500, description = "Internal error")
    )
)]
/// Drop a vector index
pub async fn drop_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Path((space_id, tag_name, field_name)): Path<(u64, String, String)>,
) -> Result<JsonResponse<VectorIndexDroppedResponse>, HttpError> {
    super::authz::require_admin_session(&state, session_id)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        vector_api
            .drop_index(space_id, &tag_name, &field_name)
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(VectorIndexDroppedResponse {
            success: true,
            message: "Vector index dropped successfully".to_string(),
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    get,
    path = "/v1/vector/indexes/{space_id}/{tag_name}/{field_name}",
    tag = "Vector",
    params(
        ("space_id" = u64, Path, description = "Space id"),
        ("tag_name" = String, Path, description = "Tag name"),
        ("field_name" = String, Path, description = "Vector field name")
    ),
    responses(
        (status = 200, body = VectorIndexDetailsResponse, description = "Vector index details"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
/// Get vector index info
pub async fn get_index_info<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Path((space_id, tag_name, field_name)): Path<(u64, String, String)>,
) -> Result<JsonResponse<VectorIndexDetailsResponse>, HttpError> {
    super::authz::require_space_read(&state, session_id, space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        match vector_api.get_index_info(space_id, &tag_name, &field_name) {
            Ok(Some(info)) => Ok(JsonResponse(VectorIndexDetailsResponse {
                // Report the engine's actual collection name instead of
                // re-deriving a display form that can drift from the real
                // naming scheme.
                collection_name: info.name.clone(),
                status: "green".to_string(),
                vectors_count: info.vector_count,
                points_count: info.vector_count,
                indexed_vectors_count: 0,
                vector_size: info.config.vector_size,
                distance: format!("{:?}", info.config.distance),
                storage: VectorStorageInfo::from_config(&info.config),
            })),
            Ok(None) => Err(HttpError::NotFound("Vector index not found".to_string())),
            Err(e) => Err(HttpError::InternalError(e.to_string())),
        }
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    get,
    operation_id = "get_v1_vector_indexes",
    path = "/v1/vector/indexes",
    tag = "Vector",
    responses(
        (status = 200, body = ListVectorIndexesResponse, description = "Vector index list"),
        (status = 500, description = "Internal error")
    )
)]
/// List all vector indexes
pub async fn list_indexes<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
) -> Result<JsonResponse<ListVectorIndexesResponse>, HttpError> {
    // The listing spans every space, so it must not expose index names to
    // callers without cross-space visibility; create/drop on this route
    // already require the admin role.
    super::authz::require_admin_session(&state, session_id)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let mut indexes = Vec::new();
        for (wrapper, meta) in vector_api.list_indexes() {
            let (vector_size, distance, points_count, storage) = match meta {
                Some(info) => (
                    info.config.vector_size,
                    format!("{:?}", info.config.distance),
                    info.vector_count,
                    Some(VectorStorageInfo::from_config(&info.config)),
                ),
                None => (0, "Cosine".to_string(), 0, None),
            };
            indexes.push(VectorIndexInfo {
                name: wrapper.collection_name,
                space_id: wrapper.space_id,
                tag_name: wrapper.tag_name,
                field_name: wrapper.field_name,
                vector_size,
                distance,
                points_count,
                status: "green".to_string(),
                storage,
            });
        }
        let count = indexes.len();
        Ok(JsonResponse(ListVectorIndexesResponse { indexes, count }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    post,
    operation_id = "post_v1_simvec",
    path = "/v1/vector/search",
    tag = "Vector",
    request_body = VectorSearchRequest,
    responses(
        (status = 200, body = VectorSearchResponse, description = "Vector search results"),
        (status = 500, description = "Internal error")
    )
)]
/// Search vectors
pub async fn search<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(request): Json<VectorSearchRequest>,
) -> Result<JsonResponse<VectorSearchResponse>, HttpError> {
    super::authz::require_space_read(&state, session_id, request.space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let mut options = SearchOptions::new(
            request.space_id,
            &request.tag_name,
            &request.field_name,
            request.query_vector,
            request.limit,
        );
        if let Some(data) = request.query_data {
            options.query_data = Some(simvec::VectorData::sparse(data.indices, data.values));
            options.query_vector.clear();
        }

        if let Some(threshold) = request.threshold {
            options = options.with_threshold(threshold);
        }

        if let Some(filter) = request.filter {
            options = options.with_filter(filter);
        }
        let _ = request.consistency.as_deref();
        let _ = request.consistency_timeout_ms;
        let _ = request.minimum_lsn;

        let results = vector_api
            .search_with_options(options)
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        let count = results.len();
        let search_results: Vec<VectorSearchResult> = results
            .into_iter()
            .map(|r| VectorSearchResult {
                doc_id: r.doc_id.to_string(),
                score: r.score,
                vector: r.vector.map(|v| v.to_vec()),
                data: r.data.map(|d| match d {
                    simvec::VectorData::Sparse { indices, values } => {
                        SparseVectorData { indices, values }
                    }
                    simvec::VectorData::Dense(_) | simvec::VectorData::Multi(_) => {
                        SparseVectorData {
                            indices: Vec::new(),
                            values: Vec::new(),
                        }
                    }
                }),
                payload: r.payload.map(|p| p.into_iter().collect()),
            })
            .collect();

        Ok(JsonResponse(VectorSearchResponse {
            results: search_results,
            count,
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    get,
    path = "/v1/vector/{space_id}/{tag_name}/{field_name}/{point_id}",
    tag = "Vector",
    params(
        ("space_id" = u64, Path, description = "Space id"),
        ("tag_name" = String, Path, description = "Tag name"),
        ("field_name" = String, Path, description = "Vector field name"),
        ("point_id" = String, Path, description = "Vector point id")
    ),
    responses(
        (status = 200, body = VectorPointResponse, description = "Vector point"),
        (status = 500, description = "Internal error")
    )
)]
/// Get vector point by ID
pub async fn get_vector<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Path((space_id, tag_name, field_name, point_id)): Path<(u64, String, String, String)>,
) -> Result<JsonResponse<VectorPointResponse>, HttpError> {
    super::authz::require_space_read(&state, session_id, space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let point = vector_api
            .get_vector(space_id, &tag_name, &field_name, &point_id)
            .await
            .map_err(|e: linkrs_api::api_core::error::CoreError| {
                HttpError::InternalError(e.to_string())
            })?;

        match point {
            Some(p) => Ok(JsonResponse(VectorPointResponse {
                success: true,
                point: Some(VectorPoint {
                    id: p.id.to_string(),
                    vector: if p.vector.is_empty() {
                        None
                    } else {
                        Some(p.vector)
                    },
                    data: match p.data {
                        Some(simvec::VectorData::Sparse { indices, values }) => {
                            Some(SparseVectorData { indices, values })
                        }
                        _ => None,
                    },
                    payload: p.payload.map(|payload| {
                        serde_json::to_value(payload)
                            .ok()
                            .and_then(|v| v.as_object().cloned())
                            .unwrap_or_default()
                    }),
                }),
                message: None,
            })),
            None => Ok(JsonResponse(VectorPointResponse {
                success: false,
                point: None,
                message: Some("Vector point not found".to_string()),
            })),
        }
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    get,
    path = "/v1/vector/{space_id}/{tag_name}/{field_name}/count",
    tag = "Vector",
    params(
        ("space_id" = u64, Path, description = "Space id"),
        ("tag_name" = String, Path, description = "Tag name"),
        ("field_name" = String, Path, description = "Vector field name")
    ),
    responses(
        (status = 200, body = VectorCountResponse, description = "Vector point count"),
        (status = 500, description = "Internal error")
    )
)]
/// Get vector index count
pub async fn count<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Path((space_id, tag_name, field_name)): Path<(u64, String, String)>,
) -> Result<JsonResponse<VectorCountResponse>, HttpError> {
    super::authz::require_space_read(&state, session_id, space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let count = vector_api
            .count(space_id, &tag_name, &field_name)
            .await
            .map_err(|e: linkrs_api::api_core::error::CoreError| {
                HttpError::InternalError(e.to_string())
            })?;

        Ok(JsonResponse(VectorCountResponse {
            success: true,
            count,
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

/// Set payload request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetPayloadRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub point_ids: Vec<String>,
    #[schema(value_type = std::collections::HashMap<String, linkrs_wire::query::CellValue>)]
    pub payload: serde_json::Map<String, serde_json::Value>,
}

/// Delete payload request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeletePayloadRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub point_ids: Vec<String>,
    pub keys: Vec<String>,
}

/// Scroll request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ScrollRequest {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub offset: Option<String>,
    pub with_payload: Option<bool>,
    pub with_vector: Option<bool>,
}

/// Scroll response
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScrollResponse {
    pub points: Vec<VectorSearchResult>,
    pub next_offset: Option<String>,
}

#[utoipa::path(
    put,
    path = "/v1/vector/payload",
    tag = "Vector",
    request_body = SetPayloadRequest,
    responses(
        (status = 200, body = PayloadMutationResponse, description = "Payload set"),
        (status = 500, description = "Internal error")
    )
)]
/// Set payload for vector points
pub async fn set_payload<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(request): Json<SetPayloadRequest>,
) -> Result<JsonResponse<PayloadMutationResponse>, HttpError> {
    super::authz::require_space_write(&state, session_id, request.space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let payload: simvec::types::Payload = request.payload.into_iter().collect();
        let point_ids: Vec<&str> = request.point_ids.iter().map(|s| s.as_str()).collect();
        vector_api
            .set_payload(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                point_ids,
                payload,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(PayloadMutationResponse {
            success: true,
            message: "Payload set successfully".to_string(),
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    put,
    path = "/v1/vector/payload/fields",
    tag = "Vector",
    request_body = SetPayloadRequest,
    responses(
        (status = 200, body = PayloadMutationResponse, description = "Payload fields merged"),
        (status = 500, description = "Internal error")
    )
)]
/// Merge fields into payload for vector points
pub async fn set_payload_fields<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(request): Json<SetPayloadRequest>,
) -> Result<JsonResponse<PayloadMutationResponse>, HttpError> {
    super::authz::require_space_write(&state, session_id, request.space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let fields: simvec::types::Payload = request.payload.into_iter().collect();
        let point_ids: Vec<&str> = request.point_ids.iter().map(|s| s.as_str()).collect();
        vector_api
            .set_payload_fields(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                point_ids,
                fields,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(PayloadMutationResponse {
            success: true,
            message: "Payload fields merged successfully".to_string(),
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    post,
    path = "/v1/vector/payload/delete",
    tag = "Vector",
    request_body = DeletePayloadRequest,
    responses(
        (status = 200, body = PayloadMutationResponse, description = "Payload keys deleted"),
        (status = 500, description = "Internal error")
    )
)]
/// Delete payload keys from vector points
pub async fn delete_payload<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(request): Json<DeletePayloadRequest>,
) -> Result<JsonResponse<PayloadMutationResponse>, HttpError> {
    super::authz::require_space_write(&state, session_id, request.space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let point_ids: Vec<&str> = request.point_ids.iter().map(|s| s.as_str()).collect();
        let keys: Vec<&str> = request.keys.iter().map(|s| s.as_str()).collect();
        vector_api
            .delete_payload(
                request.space_id,
                &request.tag_name,
                &request.field_name,
                point_ids,
                keys,
            )
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok(JsonResponse(PayloadMutationResponse {
            success: true,
            message: "Payload keys deleted successfully".to_string(),
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}

#[utoipa::path(
    post,
    path = "/v1/vector/scroll",
    tag = "Vector",
    request_body = ScrollRequest,
    responses(
        (status = 200, body = ScrollResponse, description = "Scrolled vector points"),
        (status = 500, description = "Internal error")
    )
)]
/// Paginated scroll over vector points
pub async fn scroll<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(request): Json<ScrollRequest>,
) -> Result<JsonResponse<ScrollResponse>, HttpError> {
    super::authz::require_space_read(&state, session_id, request.space_id as i64)?;
    let graph_service = state.server.get_graph_service();
    let vector_api = graph_service.vector_api();

    if let Some(vector_api) = vector_api {
        let (points, next_offset) = vector_api
            .scroll(linkrs_api::api_core::vector_api::ScrollQuery {
                space_id: request.space_id,
                tag_name: &request.tag_name,
                field_name: &request.field_name,
                limit: request.limit,
                offset: request.offset.as_deref(),
                with_payload: request.with_payload,
                with_vector: request.with_vector,
            })
            .await
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        let results: Vec<VectorSearchResult> = points
            .into_iter()
            .map(|p| VectorSearchResult {
                doc_id: p.id.to_string(),
                score: 0.0,
                vector: if request.with_vector.unwrap_or(false) && !p.vector.is_empty() {
                    Some(p.vector)
                } else {
                    None
                },
                data: match p.data {
                    Some(simvec::VectorData::Sparse { indices, values }) => {
                        Some(SparseVectorData { indices, values })
                    }
                    _ => None,
                },
                payload: p.payload.map(|pay| pay.into_iter().collect()),
            })
            .collect();

        Ok(JsonResponse(ScrollResponse {
            points: results,
            next_offset,
        }))
    } else {
        Err(HttpError::InternalError(
            "Vector API is not available".to_string(),
        ))
    }
}
