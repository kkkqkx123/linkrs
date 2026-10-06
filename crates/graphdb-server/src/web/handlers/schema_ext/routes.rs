//! Route assembly for schema extension handlers.

use axum::{
    routing::{get, post},
    Router,
};

use super::edge_type::{
    create_edge_type, delete_edge_type, get_edge_type, list_edge_types, update_edge_type,
};
use super::index::{create_index, delete_index, get_index, list_indexes, rebuild_index};
use super::space::{get_space_details, get_space_statistics, list_spaces};
use super::tag::{create_tag, delete_tag, get_tag, list_tags, update_tag};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::web::WebState;

/// Create schema extension routes (without state)
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
