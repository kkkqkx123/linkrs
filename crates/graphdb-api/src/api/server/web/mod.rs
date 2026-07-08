//! Web Management Module
//!
//! Provides Web management interface API for GraphDB frontend:
//! - Query history management
//! - Query favorites management
//! - Extended Schema management
//! - Data browsing
//! - Graph data queries

pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod services;
pub mod storage;

use axum::{middleware as axum_middleware, Router};
use std::sync::Arc;

use crate::api::server::http::state::AppState;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

use self::storage::SqliteStorage;

/// Web module state
#[derive(Clone)]
pub struct WebState<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
> {
    /// Metadata storage
    pub metadata_storage: Arc<SqliteStorage>,
    /// Core application state
    pub core_state: AppState<S>,
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageTransactionContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > WebState<S>
{
    pub async fn new(storage_path: &str, core_state: AppState<S>) -> Result<Self, error::WebError> {
        let metadata_storage = Arc::new(SqliteStorage::new(storage_path).await?);

        Ok(Self {
            metadata_storage,
            core_state,
        })
    }
}

/// Create Web management router
pub fn create_router<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    web_state: WebState<S>,
) -> Router {
    // Build routes with shared state
    let queries_routes = handlers::metadata::create_routes();
    let schema_routes = handlers::schema_ext::create_routes();
    let data_routes = handlers::data_browser::create_routes();
    let graph_routes = handlers::graph_data::create_routes();

    Router::new()
        .nest("/v1/queries", queries_routes)
        .nest("/v1/schema", schema_routes)
        .nest("/v1/data", data_routes)
        .nest("/v1/graph", graph_routes)
        .layer(axum_middleware::from_fn_with_state(
            web_state.clone(),
            middleware::web_auth_middleware,
        ))
        .with_state(web_state)
}
