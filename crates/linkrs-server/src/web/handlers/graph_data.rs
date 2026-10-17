//! Graph Data Handlers
//!
//! Provides graph data query APIs for visualization:
//! - Vertex details
//! - Edge details
//! - Neighbor queries

use axum::{
    extract::{Extension, Path, Query, State},
    response::Json,
    routing::get,
    Router,
};
use serde::Deserialize;

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::{
    error::{WebError, WebResult},
    handlers::common::{execute_in_space, vid_literal},
    models::ApiResponse,
    WebState,
};

/// Create graph data routes (without state)
pub fn create_routes<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>() -> Router<WebState<S>> {
    Router::new()
        .route("/vertices/{vid}", get(get_vertex))
        .route("/edges", get(get_edge))
        .route("/vertices/{vid}/neighbors", get(get_neighbors))
}

/// Get vertex details
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct GetVertexParams {
    pub space: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/graph/vertices/{vid}",
    tag = "WebGraph",
    params(
        ("vid" = String, Path, description = "Vertex id"),
        GetVertexParams
    ),
    responses(
        (status = 200, description = "Vertex detail", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
async fn get_vertex<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Extension(caller_session_id): Extension<i64>,
    Path(vid): Path<String>,
    Query(params): Query<GetVertexParams>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    // A vertex detail request has no tag qualifier, and the FETCH planner
    // requires one; an unlabeled MATCH scans every tag in the space.
    let id = vid_literal(&web_state, &params.space, &vid)?;
    let query = format!("MATCH (v) WHERE id(v) == {} RETURN v LIMIT 1", id);

    let result = execute_in_space(&web_state, caller_session_id, &params.space, &query).await?;
    if let Some(vertex) = result.first_value() {
        return Ok(Json(ApiResponse::success(
            serde_json::json!({"vertex": vertex}),
        )));
    }

    Err(WebError::NotFound(format!(
        "Vertex '{}' not found in space '{}'",
        vid, params.space
    )))
}

/// Get edge details
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct GetEdgeParams {
    pub space: String,
    pub src: String,
    pub dst: String,
    pub edge_type: String,
    #[serde(default)]
    pub rank: i64,
}

#[utoipa::path(
    get,
    path = "/api/v1/graph/edges",
    tag = "WebGraph",
    params(GetEdgeParams),
    responses(
        (status = 200, description = "Edge detail", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
async fn get_edge<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Extension(caller_session_id): Extension<i64>,
    Query(params): Query<GetEdgeParams>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    // Build query to fetch edge
    let query = format!(
        "FETCH PROP ON {} \"{}\" -> \"{}\"@{} YIELD edge AS e",
        params.edge_type, params.src, params.dst, params.rank
    );

    let result = execute_in_space(&web_state, caller_session_id, &params.space, &query).await?;
    if let Some(edge) = result.first_value() {
        return Ok(Json(ApiResponse::success(
            serde_json::json!({"edge": edge}),
        )));
    }

    Err(WebError::NotFound(format!(
        "Edge from '{}' to '{}' with type '{}' not found in space '{}'",
        params.src, params.dst, params.edge_type, params.space
    )))
}

/// Get neighbors of a vertex
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct GetNeighborsParams {
    pub space: String,
    /// Direction: OUT, IN, or BOTH
    #[serde(default = "default_direction")]
    pub direction: String,
    /// Edge type filter
    pub edge_type: Option<String>,
}

fn default_direction() -> String {
    "BOTH".to_string()
}

#[utoipa::path(
    get,
    path = "/api/v1/graph/vertices/{vid}/neighbors",
    tag = "WebGraph",
    params(
        ("vid" = String, Path, description = "Vertex id"),
        GetNeighborsParams
    ),
    responses(
        (status = 200, description = "Neighbor list", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
async fn get_neighbors<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Extension(caller_session_id): Extension<i64>,
    Path(vid): Path<String>,
    Query(params): Query<GetNeighborsParams>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    // An edge token carries the edge type after a colon; a bare name is read as
    // the type itself. With no filter the bracket pair matches any type.
    let edge = match &params.edge_type {
        Some(edge_type) => format!("[:{}]", edge_type),
        None => "[]".to_string(),
    };
    let pattern = match params.direction.as_str() {
        "OUT" => format!("(v)-{}->(n)", edge),
        "IN" => format!("(v)<-{}-(n)", edge),
        _ => format!("(v)-{}-(n)", edge),
    };

    let id = vid_literal(&web_state, &params.space, &vid)?;
    let query = format!("MATCH {} WHERE id(v) == {} RETURN n LIMIT 100", pattern, id);

    let result = execute_in_space(&web_state, caller_session_id, &params.space, &query).await?;
    let neighbors: Vec<serde_json::Value> = result
        .rows()
        .iter()
        .flat_map(|row| row.iter())
        .filter_map(|v| match v {
            linkrs_core::Value::Vertex(vertex) => Some(serde_json::json!({"vertex": vertex})),
            _ => None,
        })
        .collect();

    Ok(Json(ApiResponse::success(serde_json::json!({
        "vid": vid,
        "space": params.space,
        "direction": params.direction,
        "edge_type": params.edge_type,
        "neighbors": neighbors,
    }))))
}
