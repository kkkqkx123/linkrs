//! Metadata Handlers (Query History & Favorites)

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get},
    Router,
};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::{
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

/// Create metadata routes (without state)
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
#[utoipa::path(
    post,
    path = "/api/v1/queries/history",
    tag = "WebHistory",
    request_body = AddHistoryRequest,
    responses(
        (status = 200, description = "History item recorded", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
async fn add_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    get,
    path = "/api/v1/queries/history",
    tag = "WebHistory",
    params(PaginationParams),
    responses(
        (status = 200, description = "History page", body = ApiResponse<HistoryListResponse>),
        (status = 500, description = "Internal error")
    )
)]
async fn list_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    delete,
    path = "/api/v1/queries/history/{id}",
    tag = "WebHistory",
    params(("id" = String, Path, description = "History item id")),
    responses(
        (status = 200, description = "History item deleted", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
async fn delete_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    delete,
    path = "/api/v1/queries/history/clear",
    tag = "WebHistory",
    responses(
        (status = 200, description = "History cleared", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
async fn clear_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    post,
    path = "/api/v1/queries/favorites",
    tag = "WebHistory",
    request_body = AddFavoriteRequest,
    responses(
        (status = 200, description = "Favorite recorded", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
async fn add_favorite<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    get,
    path = "/api/v1/queries/favorites",
    tag = "WebHistory",
    responses(
        (status = 200, description = "Favorite list", body = ApiResponse<FavoriteListResponse>),
        (status = 500, description = "Internal error")
    )
)]
async fn list_favorites<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    get,
    path = "/api/v1/queries/favorites/{id}",
    tag = "WebHistory",
    params(("id" = String, Path, description = "Favorite id")),
    responses(
        (status = 200, description = "Favorite detail", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
async fn get_favorite<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    put,
    path = "/api/v1/queries/favorites/{id}",
    tag = "WebHistory",
    params(("id" = String, Path, description = "Favorite id")),
    request_body = UpdateFavoriteRequest,
    responses(
        (status = 200, description = "Favorite updated", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
async fn update_favorite<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    delete,
    path = "/api/v1/queries/favorites/{id}",
    tag = "WebHistory",
    params(("id" = String, Path, description = "Favorite id")),
    responses(
        (status = 200, description = "Favorite deleted", body = ApiResponse<serde_json::Value>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error")
    )
)]
async fn delete_favorite<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
#[utoipa::path(
    delete,
    path = "/api/v1/queries/favorites/clear",
    tag = "WebHistory",
    responses(
        (status = 200, description = "Favorites cleared", body = ApiResponse<serde_json::Value>),
        (status = 500, description = "Internal error")
    )
)]
async fn clear_favorites<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
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
