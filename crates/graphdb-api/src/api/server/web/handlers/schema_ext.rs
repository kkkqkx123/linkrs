//! Schema Extension Handlers
//!
//! Provides extended Schema management APIs:
//! - Space list/details/statistics
//! - Tag list/details/management
//! - Edge type list/details/management
//! - Index management

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use tokio::task;

use crate::api::server::web::{
    error::{WebError, WebResult},
    models::{
        schema::{
            CreateIndexRequest, EdgeTypeDetail, EdgeTypeSummary, IndexInfo, PropertyDef,
            SpaceDetail, SpaceStatistics, TagDetail, TagSummary, UpdateEdgeTypeRequest,
            UpdateTagRequest,
        },
        ApiResponse,
    },
    WebState,
};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Parse data type string to DataType
fn parse_data_type(type_str: &str) -> Option<crate::core::DataType> {
    match type_str.to_uppercase().as_str() {
        "BOOL" => Some(crate::core::DataType::Bool),
        "SMALLINT" | "INT16" => Some(crate::core::DataType::SmallInt),
        "INT" | "INT32" | "INTEGER" => Some(crate::core::DataType::Int),
        "BIGINT" | "INT64" => Some(crate::core::DataType::BigInt),
        "FLOAT" | "REAL" => Some(crate::core::DataType::Float),
        "DOUBLE" | "DOUBLE PRECISION" => Some(crate::core::DataType::Double),
        "STRING" => Some(crate::core::DataType::String),
        "DATE" => Some(crate::core::DataType::Date),
        "TIME" => Some(crate::core::DataType::Time),
        "DATETIME" => Some(crate::core::DataType::DateTime),
        "TIMESTAMP" => Some(crate::core::DataType::Timestamp),
        _ => None,
    }
}

/// Create schema extension routes (without state)
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
        // Space routes
        .route("/spaces", get(list_spaces))
        .route("/spaces/{name}/details", get(get_space_details))
        .route("/spaces/{name}/statistics", get(get_space_statistics))
        // Tag routes
        .route("/spaces/{name}/tags", get(list_tags).post(create_tag))
        .route(
            "/spaces/{name}/tags/{tag_name}",
            get(get_tag).put(update_tag).delete(delete_tag),
        )
        // Edge type routes
        .route(
            "/spaces/{name}/edge-types",
            get(list_edge_types).post(create_edge_type),
        )
        .route(
            "/spaces/{name}/edge-types/{edge_name}",
            get(get_edge_type)
                .put(update_edge_type)
                .delete(delete_edge_type),
        )
        // Index routes
        .route(
            "/spaces/{name}/indexes",
            get(list_indexes).post(create_index),
        )
        .route(
            "/spaces/{name}/indexes/{index_name}",
            get(get_index).delete(delete_index),
        )
        .route(
            "/spaces/{name}/indexes/{index_name}/rebuild",
            post(rebuild_index),
        )
}

// ==================== Space Handlers ====================

/// List all spaces
async fn list_spaces<
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
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    // Get storage reference before spawn_blocking
    let storage = web_state.core_state.server.get_storage();

    let result = task::spawn_blocking(move || {
        let storage = storage.read();

        let spaces = storage
            .list_spaces()
            .map_err(|e| WebError::Storage(e.to_string()))?;

        let space_list: Vec<serde_json::Value> = spaces
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.space_id,
                    "name": s.space_name,
                    "vid_type": format!("{:?}", s.vid_type),
                })
            })
            .collect();

        Ok::<_, WebError>(serde_json::json!({
            "spaces": space_list,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Get space details
async fn get_space_details<
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
    Path(name): Path<String>,
) -> WebResult<Json<ApiResponse<SpaceDetail>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        let space_info = storage
            .get_space(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", name)))?;

        // Get statistics
        let tags = storage
            .list_tags(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let edge_types = storage
            .list_edge_types(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let tag_indexes = storage
            .list_tag_indexes(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let edge_indexes: Vec<crate::core::types::Index> = Vec::new();

        Ok::<_, WebError>(SpaceDetail {
            id: space_info.space_id,
            name: space_info.space_name,
            vid_type: format!("{:?}", space_info.vid_type),
            partition_num: 100,
            replica_factor: 1,
            comment: None,
            created_at: 0,
            statistics: SpaceStatistics {
                tag_count: tags.len() as i64,
                edge_type_count: edge_types.len() as i64,
                index_count: (tag_indexes.len() + edge_indexes.len()) as i64,
                estimated_vertex_count: 0,
                estimated_edge_count: 0,
            },
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Get space statistics
async fn get_space_statistics<
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
    Path(name): Path<String>,
) -> WebResult<Json<ApiResponse<SpaceStatistics>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        // Verify space exists
        let _ = storage
            .get_space(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", name)))?;

        let tags = storage
            .list_tags(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let edge_types = storage
            .list_edge_types(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let tag_indexes = storage
            .list_tag_indexes(&name)
            .map_err(|e| WebError::Storage(e.to_string()))?;
        let edge_indexes: Vec<crate::core::types::Index> = Vec::new();

        Ok::<_, WebError>(SpaceStatistics {
            tag_count: tags.len() as i64,
            edge_type_count: edge_types.len() as i64,
            index_count: (tag_indexes.len() + edge_indexes.len()) as i64,
            estimated_vertex_count: 0,
            estimated_edge_count: 0,
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

// ==================== Tag Handlers ====================

/// List all tags in a space
async fn list_tags<
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
    Path(space_name): Path<String>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        // Verify space exists
        let _ = storage
            .get_space(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", space_name)))?;

        let tags = storage
            .list_tags(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?;

        let tag_list: Vec<TagSummary> = tags
            .into_iter()
            .enumerate()
            .map(|(idx, t)| TagSummary {
                id: idx as i64 + 1,
                name: t.tag_name,
                property_count: t.properties.len() as i64,
                index_count: 0,
                created_at: 0,
            })
            .collect();

        Ok::<_, WebError>(serde_json::json!({
            "space": space_name,
            "tags": tag_list,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Create a new tag
async fn create_tag<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path(space_name): Path<String>,
    Json(request): Json<serde_json::Value>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let tag_name = request
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WebError::BadRequest("Tag name is required".to_string()))?
        .to_string();

    // Parse properties before spawn_blocking
    let properties: Vec<crate::api::core::PropertyDef> = request
        .get("properties")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let name = p.get("name")?.as_str()?;
                    let data_type_str = p.get("data_type")?.as_str()?;
                    let data_type = parse_data_type(data_type_str)?;
                    Some(crate::api::core::PropertyDef {
                        name: name.to_string(),
                        data_type,
                        nullable: p.get("nullable").and_then(|v| v.as_bool()).unwrap_or(true),
                        default_value: None,
                        comment: p
                            .get("comment")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        // Get space ID
        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .create_tag(space_id, &tag_name, properties)
            .map_err(|e| WebError::Internal(format!("Failed to create tag: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "id": 0,
            "name": tag_name,
            "space": space_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::CREATED, Json(ApiResponse::success(result?))))
}

/// Get tag details
async fn get_tag<
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
    Path((space_name, tag_name)): Path<(String, String)>,
) -> WebResult<Json<ApiResponse<TagDetail>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        let tag_info = storage
            .get_tag(&space_name, &tag_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| {
                WebError::NotFound(format!(
                    "Tag '{}' not found in space '{}'",
                    tag_name, space_name
                ))
            })?;

        let properties: Vec<PropertyDef> = tag_info
            .properties
            .into_iter()
            .map(|p| PropertyDef {
                name: p.name,
                data_type: format!("{:?}", p.data_type),
                nullable: p.nullable,
                default_value: p.default.map(|v| format!("{:?}", v)),
            })
            .collect();

        Ok::<_, WebError>(TagDetail {
            id: 0,
            name: tag_info.tag_name,
            properties,
            indexes: vec![],
            created_at: 0,
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Update tag
async fn update_tag<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path((space_name, tag_name)): Path<(String, String)>,
    Json(request): Json<UpdateTagRequest>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        // Convert web PropertyDef to core PropertyDef
        let additions = if let Some(props) = request.add_properties {
            let mut core_props = Vec::new();
            for p in props {
                let data_type = parse_data_type(&p.data_type).ok_or_else(|| {
                    WebError::BadRequest(format!("Invalid data type: {}", p.data_type))
                })?;
                core_props.push(crate::api::core::PropertyDef {
                    name: p.name,
                    data_type,
                    nullable: p.nullable,
                    default_value: None,
                    comment: None,
                });
            }
            core_props
        } else {
            Vec::new()
        };

        let deletions = request.drop_properties.unwrap_or_default();

        schema_api
            .alter_tag(space_id, &tag_name, additions, deletions)
            .map_err(|e| WebError::Internal(format!("Failed to update tag: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "updated": true,
            "name": tag_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Delete tag
async fn delete_tag<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path((space_name, tag_name)): Path<(String, String)>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .drop_tag(space_id, &tag_name)
            .map_err(|e| WebError::Internal(format!("Failed to delete tag: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "deleted": true,
            "name": tag_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::OK, Json(ApiResponse::success(result?))))
}

// ==================== Edge Type Handlers ====================

/// List all edge types in a space
async fn list_edge_types<
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
    Path(space_name): Path<String>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        // Verify space exists
        let _ = storage
            .get_space(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", space_name)))?;

        let edge_types = storage
            .list_edge_types(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?;

        let edge_type_list: Vec<EdgeTypeSummary> = edge_types
            .into_iter()
            .enumerate()
            .map(|(idx, e)| EdgeTypeSummary {
                id: idx as i64 + 1,
                name: e.edge_type_name,
                property_count: e.properties.len() as i64,
                index_count: 0,
                created_at: 0,
            })
            .collect();

        Ok::<_, WebError>(serde_json::json!({
            "space": space_name,
            "edge_types": edge_type_list,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Create a new edge type
async fn create_edge_type<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path(space_name): Path<String>,
    Json(request): Json<serde_json::Value>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let edge_name = request
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| WebError::BadRequest("Edge type name is required".to_string()))?
        .to_string();

    // Parse properties before spawn_blocking
    let properties: Vec<crate::api::core::PropertyDef> = request
        .get("properties")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let name = p.get("name")?.as_str()?;
                    let data_type_str = p.get("data_type")?.as_str()?;
                    let data_type = parse_data_type(data_type_str)?;
                    Some(crate::api::core::PropertyDef {
                        name: name.to_string(),
                        data_type,
                        nullable: p.get("nullable").and_then(|v| v.as_bool()).unwrap_or(true),
                        default_value: None,
                        comment: p
                            .get("comment")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .create_edge_type(space_id, &edge_name, properties)
            .map_err(|e| WebError::Internal(format!("Failed to create edge type: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "id": 0,
            "name": edge_name,
            "space": space_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::CREATED, Json(ApiResponse::success(result?))))
}

/// Get edge type details
async fn get_edge_type<
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
    Path((space_name, edge_name)): Path<(String, String)>,
) -> WebResult<Json<ApiResponse<EdgeTypeDetail>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        let edge_info = storage
            .get_edge_type(&space_name, &edge_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| {
                WebError::NotFound(format!(
                    "Edge type '{}' not found in space '{}'",
                    edge_name, space_name
                ))
            })?;

        let properties: Vec<PropertyDef> = edge_info
            .properties
            .into_iter()
            .map(|p| PropertyDef {
                name: p.name,
                data_type: format!("{:?}", p.data_type),
                nullable: p.nullable,
                default_value: p.default.map(|v| format!("{:?}", v)),
            })
            .collect();

        Ok::<_, WebError>(EdgeTypeDetail {
            id: 0,
            name: edge_info.edge_type_name,
            properties,
            indexes: vec![],
            created_at: 0,
        })
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Update edge type
async fn update_edge_type<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path((space_name, edge_name)): Path<(String, String)>,
    Json(request): Json<UpdateEdgeTypeRequest>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        // Convert web PropertyDef to core PropertyDef
        let additions = if let Some(props) = request.add_properties {
            let mut core_props = Vec::new();
            for p in props {
                let data_type = parse_data_type(&p.data_type).ok_or_else(|| {
                    WebError::BadRequest(format!("Invalid data type: {}", p.data_type))
                })?;
                core_props.push(crate::api::core::PropertyDef {
                    name: p.name,
                    data_type,
                    nullable: p.nullable,
                    default_value: None,
                    comment: None,
                });
            }
            core_props
        } else {
            Vec::new()
        };

        let deletions = request.drop_properties.unwrap_or_default();

        schema_api
            .alter_edge_type(space_id, &edge_name, additions, deletions)
            .map_err(|e| WebError::Internal(format!("Failed to update edge type: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "updated": true,
            "name": edge_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Delete edge type
async fn delete_edge_type<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path((space_name, edge_name)): Path<(String, String)>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .drop_edge_type(space_id, &edge_name)
            .map_err(|e| WebError::Internal(format!("Failed to delete edge type: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "deleted": true,
            "name": edge_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::OK, Json(ApiResponse::success(result?))))
}

// ==================== Index Handlers ====================

/// List all indexes in a space
async fn list_indexes<
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
    Path(space_name): Path<String>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        // Verify space exists
        let space_info = storage
            .get_space(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
            .ok_or_else(|| WebError::NotFound(format!("Space '{}' not found", space_name)))?;

        let tag_indexes = storage
            .list_tag_indexes(&space_name)
            .map_err(|e| WebError::Storage(e.to_string()))?;

        let mut index_list: Vec<IndexInfo> = Vec::new();

        for (idx, index) in tag_indexes.iter().enumerate() {
            index_list.push(IndexInfo {
                id: idx as i64,
                name: index.name.clone(),
                index_type: format!("{:?}", index.index_type),
                fields: index.fields.iter().map(|f| f.name.clone()).collect(),
                status: format!("{:?}", index.status),
                progress: None,
                created_at: 0,
            });
        }
        Ok::<_, WebError>(serde_json::json!({
            "space": space_name,
            "space_id": space_info.space_id,
            "indexes": index_list,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Create a new index
async fn create_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path(space_name): Path<String>,
    Json(request): Json<CreateIndexRequest>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        let target = match request.entity_type.as_str() {
            "TAG" => crate::api::core::IndexTarget::Tag {
                name: request.entity_name,
                fields: request.fields,
            },
            "EDGE" => crate::api::core::IndexTarget::Edge {
                name: request.entity_name,
                fields: request.fields,
            },
            _ => {
                return Err(WebError::BadRequest(format!(
                    "Invalid entity_type: {}",
                    request.entity_type
                )))
            }
        };

        schema_api
            .create_index(space_id, &request.name, target)
            .map_err(|e| WebError::Internal(format!("Failed to create index: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "id": 0,
            "name": request.name,
            "space": space_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::CREATED, Json(ApiResponse::success(result?))))
}

/// Get index details
async fn get_index<
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
    Path((space_name, index_name)): Path<(String, String)>,
) -> WebResult<Json<ApiResponse<IndexInfo>>> {
    let result = task::spawn_blocking(move || {
        let storage = web_state.core_state.server.get_storage();
        let storage = storage.read();

        // Try to get tag index first
        if let Some(index) = storage
            .get_tag_index(&space_name, &index_name)
            .map_err(|e| WebError::Storage(e.to_string()))?
        {
            return Ok::<_, WebError>(IndexInfo {
                id: 0,
                name: index.name,
                index_type: format!("{:?}", index.index_type),
                fields: index.fields.iter().map(|f| f.name.clone()).collect(),
                status: format!("{:?}", index.status),
                progress: None,
                created_at: 0,
            });
        }

        Err(WebError::NotFound(format!(
            "Index '{}' not found in space '{}'",
            index_name, space_name
        )))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}

/// Delete index
async fn delete_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path((space_name, index_name)): Path<(String, String)>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .drop_index(space_id, &index_name)
            .map_err(|e| WebError::Internal(format!("Failed to delete index: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "deleted": true,
            "name": index_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok((StatusCode::OK, Json(ApiResponse::success(result?))))
}

/// Rebuild index
async fn rebuild_index<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(_session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path((space_name, index_name)): Path<(String, String)>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let result = task::spawn_blocking(move || {
        let schema_api = web_state.core_state.server.get_schema_api();

        let space_id = schema_api
            .use_space(&space_name)
            .map_err(|e| WebError::NotFound(format!("Space '{}' not found: {}", space_name, e)))?;

        schema_api
            .rebuild_index(space_id, &index_name)
            .map_err(|e| WebError::Internal(format!("Failed to rebuild index: {}", e)))?;

        Ok::<_, WebError>(serde_json::json!({
            "rebuilt": true,
            "name": index_name,
        }))
    })
    .await
    .map_err(|e| WebError::Internal(format!("Task execution failed: {}", e)))?;

    Ok(Json(ApiResponse::success(result?)))
}
