use axum::{
    http::StatusCode,
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::time::Duration;
use tower_http::{
    cors::{Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::storage::UndoTarget;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSnapshotOps, StorageSyncContextOps,
    StorageTransactionContextOps,
};

#[cfg(feature = "qdrant")]
use super::handlers::vector;

use super::{
    handlers::{
        auth::{login, logout},
        batch::{
            add_items, cancel as cancel_batch, create as create_batch, delete as delete_batch,
            execute as execute_batch, status as batch_status,
        },
        config::{get as get_config, get_key, reset_key, update as update_config, update_key},

        function::{info as function_info, list, register, unregister},
        health, query, schema,

        session::{create as create_session, delete_session, get_session},
        statistics::{database, freeze_stats, queries, search as search_stats, session, system, trigger_freeze},
        stream::execute_stream,
        sync, transaction,
    },
    middleware::{auth::auth_middleware, error, logging},
    state::AppState,
};

/// Creating a router
///
/// Routing structure:
/// /v1/health – Health check (public)
/// – /v1/auth/* – Related to authentication (public information)
/// – /v1/sessions/* – Session management (authentication required)
/// /v1/query – Execution of a query (authentication required)
/// /v1/transactions/* – Transaction management (authentication required)
/// – /v1/schema/* – Schema management (requires authentication)
/// – /api/* – Web management APIs (authentication required)
pub fn create_router<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + UndoTarget
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: AppState<S>,
    web_router: Option<Router>,
) -> Router {
    // Public route (no authentication required)
    let public_routes = Router::new()
        .route("/health", get(health::check))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout));

    // Routes that require authentication
    let protected_routes = Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/query", post(query::execute))
        .route("/query/validate", post(query::validate))
        .route("/transactions", post(transaction::begin))
        .route("/transactions/{id}/commit", post(transaction::commit))
        .route("/transactions/{id}/rollback", post(transaction::rollback))
        .route(
            "/transactions/{id}/savepoints",
            post(transaction::create_savepoint).get(transaction::get_savepoints),
        )
        .route(
            "/transactions/{id}/savepoints/{sid}/rollback",
            post(transaction::rollback_to_savepoint),
        )
        .route(
            "/transactions/{id}/savepoints/{sid}",
            delete(transaction::release_savepoint),
        )
        // Batch operation of routes
        .route("/batch", post(create_batch))
        .route("/batch/{id}", get(batch_status).delete(delete_batch))
        .route("/batch/{id}/items", post(add_items))
        .route("/batch/{id}/execute", post(execute_batch))
         .route("/batch/{id}/cancel", post(cancel_batch))
         // Import/Export routes
         .route("/import", post(super::handlers::import::import_file))
         .route("/import/{id}", get(super::handlers::import::import_status))
         .route("/export", get(super::handlers::export::export_data))
         // Statistical information routing
        .route("/statistics/sessions/{id}", get(session))
        .route("/statistics/queries", get(queries))
        .route("/statistics/database", get(database))
        .route("/statistics/system", get(system))
         .route("/statistics/search", get(search_stats))
         .route("/statistics/freeze", get(freeze_stats).post(trigger_freeze))
        // Configure management routing.
        .route("/config", get(get_config).put(update_config))
        .route(
            "/config/{section}/{key}",
            get(get_key).put(update_key).delete(reset_key),
        )
        // Custom function routing
        .route("/functions", post(register).get(list))
        .route("/functions/{name}", get(function_info).delete(unregister))
        // Streaming Query Routing
        .route("/query/stream", post(execute_stream))
        // Sync Management Routes
        .route("/sync/status", get(sync::status))
        // Schema Routes
        .route(
            "/schema/spaces",
            post(schema::create_space).get(schema::list_spaces),
        )
        .route(
            "/schema/spaces/{name}",
            get(schema::get_space).delete(schema::drop_space),
        )
        .route(
            "/schema/spaces/{name}/tags",
            post(schema::create_tag).get(schema::list_tags),
        )
        .route(
            "/schema/spaces/{name}/edge-types",
            post(schema::create_edge_type).get(schema::list_edge_types),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let protected_routes = add_vector_routes(protected_routes);

    let router = Router::new()
        .nest("/v1", public_routes.merge(protected_routes))
        .layer(middleware::from_fn(logging::logging_middleware))
        .layer(middleware::from_fn(error::error_handling_middleware))
        .layer(TraceLayer::new_for_http())
        .layer(create_cors_layer())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(RequestBodyLimitLayer::new(1024 * 1024 * 10)) // Limit on the request body size: 10 MB
        .with_state(state);

    // Add web management routes if web_router is provided
    if let Some(wr) = web_router {
        router.nest("/api", wr)
    } else {
        router
    }
}

/// Conditionally add vector search routes
#[cfg(feature = "qdrant")]
fn add_vector_routes<
    S: crate::storage::StorageClient
        + crate::storage::StorageSchemaContextOps
        + crate::storage::StorageSyncContextOps
        + crate::storage::StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    router: Router<super::state::AppState<S>>,
) -> Router<super::state::AppState<S>> {
    router
        .route(
            "/vector/indexes",
            post(vector::create_index).get(vector::list_indexes),
        )
        .route(
            "/vector/indexes/{space_id}/{tag_name}/{field_name}",
            get(vector::get_index_info).delete(vector::drop_index),
        )
        .route("/vector/search", post(vector::search))
        .route(
            "/vector/{space_id}/{tag_name}/{field_name}/{point_id}",
            get(vector::get_vector),
        )
        .route(
            "/vector/{space_id}/{tag_name}/{field_name}/count",
            get(vector::count),
        )
}

#[cfg(not(feature = "qdrant"))]
fn add_vector_routes<
    S: crate::storage::StorageClient
        + crate::storage::StorageSchemaContextOps
        + crate::storage::StorageSyncContextOps
        + crate::storage::StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    router: Router<super::state::AppState<S>>,
) -> Router<super::state::AppState<S>> {
    router
}

/// Create a CORS configuration layer
///
/// The development environment allows all sources; the production environment should be configured with specific sources.
fn create_cors_layer() -> CorsLayer {
    // The configuration should be tightened in a production environment.
    // For example: Access is only allowed from specific domain names.
    CorsLayer::new()
        .allow_origin(Any) // Allow all sources; the production environment should be replaced with specific domain names.
        .allow_methods(Any)
        .allow_headers(Any)
}
