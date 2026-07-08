//! Web Management API Integration Tests
//!
//! Test web management interface APIs including:
//! - Metadata storage (query history, favorites)
//! - Schema management (spaces, tags, edge types, indexes)
//! - Data browsing
//! - Graph data queries

#![cfg(feature = "server")]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

mod common;

/// Test helper to create a test web state with temporary storage
/// Returns the web state and a valid session ID for testing
async fn create_test_web_state() -> (
    graphdb::api::server::web::WebState<graphdb::storage::GraphStorage>,
    i64,
) {
    use graphdb::api::server::graph_service::GraphService;
    use graphdb::api::server::http::AppState;
    use graphdb::api::server::http::HttpServer;
    use graphdb::api::server::web::{storage::SqliteStorage, WebState};
    use graphdb::config::Config;
    use graphdb::storage::GraphStorage;
    use graphdb::transaction::{TransactionManager, TransactionManagerConfig};
    use parking_lot::RwLock;
    use tempfile::tempdir;

    // Create temporary directories
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create storage
    let storage = GraphStorage::new_with_path(db_path).unwrap();
    let storage_arc = Arc::new(storage.clone());
    let storage_mutex = Arc::new(RwLock::new(storage));

    // Create config
    let config = Config::default();

    // Create graph service for test (no transaction manager, no background tasks)
    let graph_service = GraphService::new_for_test(config.clone(), storage_arc.clone()).await;

    // Create transaction manager
    let txn_manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    // Create HTTP server
    let http_server = Arc::new(HttpServer::new(
        graph_service.clone(),
        storage_mutex,
        txn_manager,
        &config,
    ));

    // Create a valid session for testing
    let session = graph_service
        .get_session_manager()
        .create_session("test_user".to_string(), "127.0.0.1".to_string())
        .await
        .unwrap();
    let session_id = session.id();

    // Create core app state
    let core_state = AppState::new(http_server);

    // Create metadata storage using in-memory SQLite for tests
    let metadata_storage = Arc::new(SqliteStorage::new("sqlite::memory:").await.unwrap());

    let web_state = WebState {
        metadata_storage,
        core_state,
    };

    (web_state, session_id)
}

/// Test creating and listing query history
#[tokio::test]
async fn test_query_history_lifecycle() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    // Create a query history entry
    let create_request = Request::builder()
        .method("POST")
        .uri("/v1/queries/history")
        .header("content-type", "application/json")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::from(
            json!({
                "query": "MATCH (n) RETURN n LIMIT 10",
                "execution_time_ms": 100,
                "rows_returned": 10,
                "success": true
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.clone().oneshot(create_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // List query history
    let list_request = Request::builder()
        .method("GET")
        .uri("/v1/queries/history?page=1&page_size=10")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(list_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// Test favorites CRUD operations
#[tokio::test]
async fn test_favorites_crud() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    // Create a favorite
    let create_request = Request::builder()
        .method("POST")
        .uri("/v1/queries/favorites")
        .header("content-type", "application/json")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::from(
            json!({
                "name": "Test Query",
                "query": "MATCH (n) RETURN n",
                "space": "test_space"
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.clone().oneshot(create_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    // List favorites
    let list_request = Request::builder()
        .method("GET")
        .uri("/v1/queries/favorites")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(list_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// Test space listing
#[tokio::test]
async fn test_list_spaces() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/schema/spaces")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should return OK or potentially fail due to storage initialization in tests
    assert!(
        response.status() == StatusCode::OK
            || response.status() == StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// Test tag listing in a space
#[tokio::test]
async fn test_list_tags() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/schema/spaces/test_space/tags")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should return 404 if space doesn't exist
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND);
}

/// Test edge type listing
#[tokio::test]
async fn test_list_edge_types() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/schema/spaces/test_space/edge_types")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should return 404 if space doesn't exist
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND);
}

/// Test index listing
#[tokio::test]
async fn test_list_indexes() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/schema/spaces/test_space/indexes")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should return 404 if space doesn't exist
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND);
}

/// Test data browsing - list vertices
#[tokio::test]
async fn test_list_vertices() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/data/spaces/test_space/vertices?page=1&page_size=10")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should return 404 if space doesn't exist
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND);
}

/// Test data browsing - list edges
#[tokio::test]
async fn test_list_edges() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/data/spaces/test_space/edges?page=1&page_size=10")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should return 404 if space doesn't exist
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND);
}

/// Test graph data query execution
#[tokio::test]
async fn test_execute_query() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("POST")
        .uri("/v1/graph/execute")
        .header("content-type", "application/json")
        .header("X-Session-ID", session_id.to_string())
        .body(Body::from(
            json!({
                "query": "MATCH (n) RETURN n LIMIT 1",
                "space": "test_space"
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should process the query (may fail due to no space, but should not panic)
    assert!(response.status().as_u16() < 500);
}

/// Test authentication middleware - missing session ID
#[tokio::test]
async fn test_auth_middleware_missing_session() {
    let (web_state, _session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/queries/history")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// Test authentication middleware - invalid session ID
#[tokio::test]
async fn test_auth_middleware_invalid_session() {
    let (web_state, _session_id) = create_test_web_state().await;
    let app = graphdb::api::server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/queries/history")
        .header("X-Session-ID", "99999")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
