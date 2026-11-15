//! Single-host bundle contract: the API router and the static file router
//! merge side by side on one port.
//!
//! Pages and fingerprinted assets come from the static directory, API routes
//! keep their precedence, and unknown paths resolve per the SPA fallback
//! rules instead of leaking the index page for missing assets.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use linkrs::config::Config;
use linkrs::storage::{GraphStorage, PropertyGraphConfig};
use linkrs::transaction::{TransactionManager, TransactionManagerConfig};
use linkrs_server::http::router::create_router;
use linkrs_server::http::server::HttpServer;
use linkrs_server::http::state::AppState;
use linkrs_server::GraphService;
use parking_lot::RwLock;
use tower::ServiceExt;

fn fixture_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("static fixture dir");
    std::fs::create_dir_all(dir.path().join("_app/immutable")).expect("fixture dirs");
    std::fs::write(dir.path().join("index.html"), "<html>single</html>").expect("index");
    std::fs::write(
        dir.path().join("_app/immutable/app.abc123.js"),
        "console.log(1)",
    )
    .expect("asset");
    dir
}

async fn build_app(config: Config) -> Router {
    let storage = Arc::new(
        GraphStorage::new_with_config(PropertyGraphConfig::test()).expect("in-memory storage"),
    );
    let graph_service: Arc<GraphService<GraphStorage>> =
        GraphService::new_for_test(config.clone(), storage.clone()).await;
    let txn_manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));
    let http_server = Arc::new(HttpServer::new(
        graph_service,
        Arc::new(RwLock::new((*storage).clone())),
        txn_manager,
        &config,
        None,
    ));
    let state = AppState::new(http_server);
    create_router(state, None).expect("router builds")
}

fn request(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(""))
        .expect("test request")
}

fn get(uri: &str) -> Request<Body> {
    request(Method::GET, uri)
}

fn enabled_config(dir: &tempfile::TempDir) -> Config {
    let mut config = Config::default();
    config.server.auth.enable_authorize = false;
    config.server.http.static_dir = Some(dir.path().to_string_lossy().into_owned());
    config
}

#[tokio::test]
async fn single_host_serves_pages_beside_api() {
    let dir = fixture_dir();
    let app = build_app(enabled_config(&dir)).await;

    let response = app.clone().oneshot(get("/")).await.expect("root page");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(get("/dashboard"))
        .await
        .expect("spa path");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(get("/_app/immutable/app.abc123.js"))
        .await
        .expect("fingerprinted asset");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("cache-control")
        .is_some_and(|v| v.to_str().unwrap_or_default().contains("immutable")));

    let response = app
        .clone()
        .oneshot(get("/missing.css"))
        .await
        .expect("missing asset");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(get("/v1/no-such-route"))
        .await
        .expect("unknown api");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(request(Method::POST, "/dashboard"))
        .await
        .expect("non-read method");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    // The metrics endpoint has no auth or readiness gate, so it proves the
    // API routes still answer beside the static surface.
    let response = app.oneshot(get("/v1/metrics")).await.expect("metrics");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn disabled_static_dir_keeps_pure_api_router() {
    let mut config = Config::default();
    config.server.auth.enable_authorize = false;
    let app = build_app(config).await;

    let response = app.clone().oneshot(get("/dashboard")).await.expect("page");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app.oneshot(get("/v1/metrics")).await.expect("metrics");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn enabled_static_dir_with_missing_dir_refuses_router() {
    let storage = Arc::new(
        GraphStorage::new_with_config(PropertyGraphConfig::test()).expect("in-memory storage"),
    );
    let mut config = Config::default();
    config.server.auth.enable_authorize = false;
    config.server.http.static_dir = Some("/definitely/missing/linkrs-web-dir".to_string());
    let graph_service: Arc<GraphService<GraphStorage>> =
        GraphService::new_for_test(config.clone(), storage.clone()).await;
    let txn_manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));
    let http_server = Arc::new(HttpServer::new(
        graph_service,
        Arc::new(RwLock::new((*storage).clone())),
        txn_manager,
        &config,
        None,
    ));
    let state = AppState::new(http_server);
    assert!(
        create_router(state, None).is_err(),
        "unusable static dir must fail router construction"
    );
}

#[tokio::test]
async fn enabled_static_dir_with_missing_fallback_page_refuses_router() {
    let dir = tempfile::tempdir().expect("static fixture dir");
    std::fs::write(dir.path().join("app.js"), "console.log(1)").expect("asset");
    let storage = Arc::new(
        GraphStorage::new_with_config(PropertyGraphConfig::test()).expect("in-memory storage"),
    );
    let mut config = Config::default();
    config.server.auth.enable_authorize = false;
    config.server.http.static_dir = Some(dir.path().to_string_lossy().into_owned());
    let graph_service: Arc<GraphService<GraphStorage>> =
        GraphService::new_for_test(config.clone(), storage.clone()).await;
    let txn_manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));
    let http_server = Arc::new(HttpServer::new(
        graph_service,
        Arc::new(RwLock::new((*storage).clone())),
        txn_manager,
        &config,
        None,
    ));
    let state = AppState::new(http_server);
    assert!(
        create_router(state, None).is_err(),
        "missing fallback page must fail router construction"
    );
}

#[tokio::test]
async fn disabled_fallback_keeps_page_paths_missing_beside_api() {
    let dir = fixture_dir();
    let mut config = enabled_config(&dir);
    config.server.http.static_fallback = false;
    let app = build_app(config).await;

    let response = app.clone().oneshot(get("/dashboard")).await.expect("page");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app.clone().oneshot(get("/")).await.expect("root");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(get("/_app/immutable/app.abc123.js"))
        .await
        .expect("fingerprinted asset");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app.oneshot(get("/v1/metrics")).await.expect("metrics");
    assert_eq!(response.status(), StatusCode::OK);
}
