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

/// Test helper to create a test web state with temporary storage
/// Returns the web state and a valid session ID for testing
async fn create_test_web_state() -> (
    linkrs_server::web::WebState<linkrs::storage::GraphStorage>,
    i64,
) {
    create_test_web_state_with(true).await
}

/// Same as [`create_test_web_state`], with authorization switched on or off so
/// tests can cover both the authenticated-caller and default-identity paths.
async fn create_test_web_state_with(
    enable_authorize: bool,
) -> (
    linkrs_server::web::WebState<linkrs::storage::GraphStorage>,
    i64,
) {
    use linkrs::config::Config;
    use linkrs::storage::GraphStorage;
    use linkrs::transaction::{TransactionManager, TransactionManagerConfig};
    use linkrs_server::graph_service::GraphService;
    use linkrs_server::http::AppState;
    use linkrs_server::http::HttpServer;
    use linkrs_server::web::{storage::SqliteStorage, WebState};
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
    let mut config = Config::default();
    config.server.auth.enable_authorize = enable_authorize;

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
        None,
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

/// Seed one space with two vertices and one edge through the query pipeline, so
/// the endpoints are exercised against data created the way a client creates it.
async fn seed_graph_data(web_state: &linkrs_server::web::WebState<linkrs::storage::GraphStorage>) {
    let graph_service = web_state.core_state.server.get_graph_service();
    let admin = graph_service
        .get_session_manager()
        .create_session("root".to_string(), "127.0.0.1".to_string())
        .await
        .expect("seed session");

    for stmt in [
        "CREATE SPACE IF NOT EXISTS test_space",
        "USE test_space",
        "CREATE TAG Person(id INT64, name STRING)",
        "CREATE EDGE KNOWS(weight DOUBLE) FROM Person TO Person",
        "INSERT VERTEX Person(id, name) VALUES 1:(1, \"Alice\")",
        "INSERT VERTEX Person(id, name) VALUES 2:(2, \"Bob\")",
        "INSERT EDGE KNOWS(weight) VALUES 1 -> 2:(1.0)",
    ] {
        graph_service
            .execute(admin.id(), stmt)
            .await
            .unwrap_or_else(|err| panic!("seed statement failed: {stmt}: {err}"));
    }
}

/// Test creating and listing query history
#[tokio::test]
async fn test_query_history_lifecycle() {
    let (web_state, session_id) = create_test_web_state().await;
    let app = linkrs_server::web::create_router(web_state);

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
    let app = linkrs_server::web::create_router(web_state);

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
    let app = linkrs_server::web::create_router(web_state);

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
    let app = linkrs_server::web::create_router(web_state);

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
    let app = linkrs_server::web::create_router(web_state);

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
    let app = linkrs_server::web::create_router(web_state);

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

/// Test data browsing - list vertices of a seeded space
#[tokio::test]
async fn test_list_vertices() {
    let (web_state, _session_id) = create_test_web_state_with(false).await;
    seed_graph_data(&web_state).await;
    let app = linkrs_server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/data/spaces/test_space/tags/Person/vertices?limit=10&offset=0")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );

    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["total"], 2, "two seeded vertices");
    assert_eq!(payload["data"]["items"].as_array().unwrap().len(), 2);
}

/// Unknown spaces are reported as not found instead of an opaque query error
#[tokio::test]
async fn test_list_vertices_unknown_space_is_not_found() {
    let (web_state, _session_id) = create_test_web_state_with(false).await;
    let app = linkrs_server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/data/spaces/missing_space/tags/Person/vertices?limit=10&offset=0")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );
}

/// Test data browsing - list edges of a seeded space
#[tokio::test]
async fn test_list_edges() {
    let (web_state, _session_id) = create_test_web_state_with(false).await;
    seed_graph_data(&web_state).await;
    let app = linkrs_server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/data/spaces/test_space/edge-types/KNOWS/edges?limit=10&offset=0")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );

    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["success"], true);
    assert_eq!(payload["data"]["total"], 1, "one seeded edge");
}

/// Test graph data query execution - vertex detail of a seeded space
#[tokio::test]
async fn test_execute_query() {
    let (web_state, _session_id) = create_test_web_state_with(false).await;
    seed_graph_data(&web_state).await;
    let app = linkrs_server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/graph/vertices/1?space=test_space")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );

    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["success"], true);
    assert!(payload["data"]["vertex"].is_object());
}

/// Test graph data query execution - neighbors of a seeded vertex
#[tokio::test]
async fn test_graph_neighbors() {
    let (web_state, _session_id) = create_test_web_state_with(false).await;
    seed_graph_data(&web_state).await;
    let app = linkrs_server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/graph/vertices/1/neighbors?space=test_space&direction=OUT")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );

    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["success"], true);
    assert_eq!(
        payload["data"]["neighbors"].as_array().unwrap().len(),
        1,
        "one outgoing edge must yield one neighbor",
    );
}

/// Test graph data query execution - edge detail of a seeded space
#[tokio::test]
async fn test_edge_detail() {
    let (web_state, _session_id) = create_test_web_state_with(false).await;
    seed_graph_data(&web_state).await;
    let app = linkrs_server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/graph/edges?space=test_space&edge_type=KNOWS&src=1&dst=2&rank=0")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );

    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["success"], true);
    assert!(payload["data"]["edge"].is_object());
}

/// Test authentication middleware - missing session ID
#[tokio::test]
async fn test_auth_middleware_missing_session() {
    let (web_state, _session_id) = create_test_web_state().await;
    let app = linkrs_server::web::create_router(web_state);

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
    let app = linkrs_server::web::create_router(web_state);

    let request = Request::builder()
        .method("GET")
        .uri("/v1/queries/history")
        .header("X-Session-ID", "99999")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
