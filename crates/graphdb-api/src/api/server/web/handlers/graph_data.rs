//! Graph Data Handlers
//!
//! Provides graph data query APIs for visualization:
//! - Vertex details
//! - Edge details
//! - Neighbor queries

use axum::{
    extract::{Path, Query, State},
    response::Json,
    routing::get,
    Router,
};
use serde::Deserialize;

use crate::api::server::web::{
    error::{WebError, WebResult},
    models::ApiResponse,
    WebState,
};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Create graph data routes (without state)
pub fn create_routes<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
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
#[derive(Debug, Deserialize)]
pub struct GetVertexParams {
    pub space: String,
}

async fn get_vertex<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Path(vid): Path<String>,
    Query(params): Query<GetVertexParams>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let graph_service = web_state.core_state.server.get_graph_service();

    // Build query to fetch vertex by ID
    let query = format!(
        "USE {}; FETCH PROP ON * \"{}\" YIELD vertex AS v",
        params.space, vid
    );

    let result = match graph_service.execute(0, &query).await {
        Ok(exec_result) => match exec_result {
            crate::query::executor::ExecutionResult::DataSet(ds) => {
                if let Some(row) = ds.rows.first() {
                    if let Some(vertex) = row.first() {
                        Ok(serde_json::json!({"vertex": vertex}))
                    } else {
                        Err(WebError::NotFound(format!(
                            "Vertex '{}' not found in space '{}'",
                            vid, params.space
                        )))
                    }
                } else {
                    Err(WebError::NotFound(format!(
                        "Vertex '{}' not found in space '{}'",
                        vid, params.space
                    )))
                }
            }
            _ => Err(WebError::NotFound(format!(
                "Vertex '{}' not found in space '{}'",
                vid, params.space
            ))),
        },
        Err(e) => Err(WebError::Query(format!("Failed to get vertex: {}", e))),
    };

    Ok(Json(ApiResponse::success(result?)))
}

/// Get edge details
#[derive(Debug, Deserialize)]
pub struct GetEdgeParams {
    pub space: String,
    pub src: String,
    pub dst: String,
    pub edge_type: String,
    #[serde(default)]
    pub rank: i64,
}

async fn get_edge<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Query(params): Query<GetEdgeParams>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let graph_service = web_state.core_state.server.get_graph_service();

    // Build query to fetch edge
    let query = format!(
        "USE {}; FETCH PROP ON {} \"{}\" -> \"{}\"@{} YIELD edge AS e",
        params.space, params.edge_type, params.src, params.dst, params.rank
    );

    let result = match graph_service.execute(0, &query).await {
        Ok(exec_result) => match exec_result {
            crate::query::executor::ExecutionResult::DataSet(ds) => {
                if let Some(row) = ds.rows.first() {
                    if let Some(edge) = row.first() {
                        Ok(serde_json::json!({"edge": edge}))
                    } else {
                        Err(WebError::NotFound(format!(
                            "Edge from '{}' to '{}' with type '{}' not found in space '{}'",
                            params.src, params.dst, params.edge_type, params.space
                        )))
                    }
                } else {
                    Err(WebError::NotFound(format!(
                        "Edge from '{}' to '{}' with type '{}' not found in space '{}'",
                        params.src, params.dst, params.edge_type, params.space
                    )))
                }
            }
            _ => Err(WebError::NotFound(format!(
                "Edge from '{}' to '{}' with type '{}' not found in space '{}'",
                params.src, params.dst, params.edge_type, params.space
            ))),
        },
        Err(e) => Err(WebError::Query(format!("Failed to get edge: {}", e))),
    };

    Ok(Json(ApiResponse::success(result?)))
}

/// Get neighbors of a vertex
#[derive(Debug, Deserialize)]
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

async fn get_neighbors<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(web_state): State<WebState<S>>,
    Path(vid): Path<String>,
    Query(params): Query<GetNeighborsParams>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let graph_service = web_state.core_state.server.get_graph_service();

    // Build match pattern based on direction
    let pattern = match params.direction.as_str() {
        "OUT" => "(v)-[e]->(n)".to_string(),
        "IN" => "(v)<-[e]-(n)".to_string(),
        _ => "(v)-[e]-(n)".to_string(),
    };

    // Add edge type filter if specified
    let edge_filter = params
        .edge_type
        .as_ref()
        .map(|et| format!(":{}", et))
        .unwrap_or_default();
    let pattern = pattern.replace("[e]", &format!("[e{}]", edge_filter));

    let query = format!(
        "USE {}; MATCH {} WHERE id(v) == \"{}\" RETURN n LIMIT 100",
        params.space, pattern, vid
    );

    let result = match graph_service.execute(0, &query).await {
        Ok(exec_result) => {
            let neighbors: Vec<serde_json::Value> = match exec_result {
                crate::query::executor::ExecutionResult::DataSet(dataset) => dataset
                    .rows
                    .into_iter()
                    .flat_map(|row| row.into_iter())
                    .filter_map(|v| match v {
                        crate::core::Value::Vertex(vertex) => {
                            Some(serde_json::json!({"vertex": vertex}))
                        }
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            };

            Ok(serde_json::json!({
                "vid": vid,
                "space": params.space,
                "direction": params.direction,
                "edge_type": params.edge_type,
                "neighbors": neighbors,
            }))
        }
        Err(e) => Err(WebError::Query(format!("Failed to get neighbors: {}", e))),
    };

    Ok(Json(ApiResponse::success(result?)))
}
