//! Metadata Handlers (Query History & Favorites)

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get},
    Router,
};

use crate::api::server::web::{
    error::WebResult,
    models::{
        metadata::{
            AddFavoriteRequest, AddHistoryRequest, FavoriteListResponse, HistoryListResponse,
            UpdateFavoriteRequest,
        },
        ApiResponse, PaginationParams,
    },
    services::metadata_service::MetadataService,
    WebState,
};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Create metadata routes (without state)
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
        .route("/history", get(list_history).post(add_history))
        .route("/history/{id}", delete(delete_history))
        .route("/history/clear", delete(clear_history))
        .route("/favorites", get(list_favorites).post(add_favorite))
        .route(
            "/favorites/{id}",
            get(get_favorite)
                .put(update_favorite)
                .delete(delete_favorite),
        )
        .route("/favorites/clear", delete(clear_favorites))
}

/// Add a query history item
async fn add_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Json(request): Json<AddHistoryRequest>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    let item = service.add_history(&session_id_str, request).await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::success(serde_json::json!({
            "id": item.id,
            "query": item.query,
            "executed_at": item.executed_at,
            "execution_time_ms": item.execution_time_ms,
            "rows_returned": item.rows_returned,
            "success": item.success,
        }))),
    ))
}

/// List query history
async fn list_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Query(params): Query<PaginationParams>,
) -> WebResult<Json<ApiResponse<HistoryListResponse>>> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    let (items, total) = service
        .get_history(&session_id_str, params.limit, params.offset)
        .await?;

    Ok(Json(ApiResponse::success(HistoryListResponse {
        items,
        total,
    })))
}

/// Delete a history item
async fn delete_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path(id): Path<String>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    service.delete_history(&id, &session_id_str).await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::success(serde_json::json!({"deleted": true}))),
    ))
}

/// Clear all history
async fn clear_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    service.clear_history(&session_id_str).await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::success(serde_json::json!({"cleared": true}))),
    ))
}

/// Add a favorite
async fn add_favorite<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Json(request): Json<AddFavoriteRequest>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    let item = service.add_favorite(&session_id_str, request).await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::success(serde_json::json!({
            "id": item.id,
            "name": item.name,
            "query": item.query,
            "description": item.description,
            "created_at": item.created_at,
        }))),
    ))
}

/// List all favorites
async fn list_favorites<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
) -> WebResult<Json<ApiResponse<FavoriteListResponse>>> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    let items = service.get_favorites(&session_id_str).await?;

    Ok(Json(ApiResponse::success(FavoriteListResponse { items })))
}

/// Get a favorite by ID
async fn get_favorite<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path(id): Path<String>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    let item = service.get_favorite(&id, &session_id_str).await?;

    Ok(Json(ApiResponse::success(serde_json::json!({
        "id": item.id,
        "name": item.name,
        "query": item.query,
        "description": item.description,
        "created_at": item.created_at,
    }))))
}

/// Update a favorite
async fn update_favorite<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path(id): Path<String>,
    Json(request): Json<UpdateFavoriteRequest>,
) -> WebResult<Json<ApiResponse<serde_json::Value>>> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    let item = service
        .update_favorite(&id, &session_id_str, request)
        .await?;

    Ok(Json(ApiResponse::success(serde_json::json!({
        "id": item.id,
        "name": item.name,
        "query": item.query,
        "description": item.description,
        "created_at": item.created_at,
    }))))
}

/// Delete a favorite
async fn delete_favorite<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
    Path(id): Path<String>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    service.delete_favorite(&id, &session_id_str).await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::success(serde_json::json!({"deleted": true}))),
    ))
}

/// Clear all favorites
async fn clear_favorites<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    Extension(session_id): Extension<i64>,
    State(web_state): State<WebState<S>>,
) -> WebResult<(StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let session_id_str = session_id.to_string();

    let service = MetadataService::new(web_state.metadata_storage.clone());
    service.delete_all_favorites(&session_id_str).await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::success(serde_json::json!({"cleared": true}))),
    ))
}
