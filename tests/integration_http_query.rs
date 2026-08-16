//! HTTP materialized query endpoint integration tests.
//!
//! Verifies the `/v1/query` response envelope carries the engine's real
//! execution metadata (rows_scanned / rows_returned / space_id) instead of
//! the hardcoded zeros of the old ExecutionResult round trip.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    routing::post,
    Router,
};
use graphdb::api::server::http::handlers::execute;
use graphdb::api::server::http::server::HttpServer;
use graphdb::api::server::http::state::AppState;
use graphdb::api::server::GraphService;
use graphdb::config::Config;
use graphdb::core::types::{SpaceInfo, SpaceSummary, VertexId};
use graphdb::core::vertex_edge_path::{Tag, Vertex};
use graphdb::core::{DataType, Value};
use graphdb::storage::{GraphStorage, PropertyGraphConfig, StorageSchemaOps, StorageWriter};
use graphdb::transaction::{TransactionManager, TransactionManagerConfig};
use parking_lot::RwLock;
use tower::ServiceExt;

fn setup_test_data(storage: &Arc<RwLock<GraphStorage>>) -> SpaceSummary {
    let mut store = storage.write();
    let mut space = SpaceInfo::new("test".to_string()).with_vid_type(DataType::BigInt);
    store.create_space(&mut space).unwrap();
    let tag = graphdb::core::types::TagInfo::new("Person".to_string()).with_properties(vec![
        graphdb::core::types::PropertyDef::new("name".to_string(), DataType::String),
        graphdb::core::types::PropertyDef::new("age".to_string(), DataType::BigInt),
    ]);
    store.create_tag("test", &tag).unwrap();
    for (i, (name, age)) in [
        ("Alice", 30i64),
        ("Bob", 25),
        ("Charlie", 35),
        ("Diana", 28),
    ]
    .iter()
    .enumerate()
    {
        let vid = VertexId::from_int64(i as i64 + 1);
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::string(name));
        props.insert("age".to_string(), Value::BigInt(*age));
        let vertex = Vertex::new(vid, vec![Tag::new("Person".to_string(), props)]);
        store.insert_vertex("test", vertex).unwrap();
    }
    SpaceSummary::from(&space)
}

async fn build_query_app() -> (Router, i64) {
    let storage = Arc::new(
        GraphStorage::new_with_config(PropertyGraphConfig::test()).expect("in-memory storage"),
    );
    let storage_rwlock = Arc::new(RwLock::new((*storage).clone()));
    let test_space = setup_test_data(&storage_rwlock);

    let mut config = Config::default();
    config.server.auth.enable_authorize = false;
    let graph_service: Arc<GraphService<GraphStorage>> =
        GraphService::new_for_test(config.clone(), storage.clone()).await;

    let session = graph_service
        .authenticate("root", "root")
        .await
        .expect("root auth should succeed");
    let session_id = session.id();
    session.set_space(test_space);

    let txn_manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));
    let http_server = Arc::new(HttpServer::new(
        graph_service,
        storage_rwlock.clone(),
        txn_manager,
        &config,
    ));
    let state = AppState::new(http_server);

    let app = Router::new()
        .route("/v1/query", post(execute::<GraphStorage>))
        .with_state(state);

    (app, session_id)
}

async fn post_query(app: &Router, session_id: i64, query: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "query": query,
        "session_id": session_id,
    })
    .to_string();

    let request = Request::builder()
        .uri("/v1/query")
        .method(Method::POST)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body"),
    )
    .expect("response must be JSON")
}

#[tokio::test]
async fn query_response_carries_real_execution_metadata() {
    let (app, session_id) = build_query_app().await;

    let json = post_query(
        &app,
        session_id,
        "MATCH (n:Person) RETURN n.name, n.age ORDER BY n.name",
    )
    .await;

    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["row_count"], 4);
    assert_eq!(json["data"]["columns"].as_array().unwrap().len(), 2);
    // Engine metadata is surfaced (rows_scanned / rows_returned match the
    // materialized row count; they were hardcoded to 0 before the fix).
    let metadata = &json["metadata"];
    assert_eq!(metadata["rows_scanned"], 4);
    assert_eq!(metadata["rows_returned"], 4);
    assert_eq!(metadata["space_id"], serde_json::Value::Null);
}

#[tokio::test]
async fn query_response_use_statement_surfaces_space_id() {
    let (app, session_id) = build_query_app().await;

    let json = post_query(&app, session_id, "USE test").await;

    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["row_count"], 1);
    assert_eq!(json["data"]["columns"], serde_json::json!(["space_name", "space_id", "vid_type"]));
    assert_eq!(json["data"]["rows"][0]["space_name"], "test");
    assert!(json["metadata"]["space_id"].is_number());
}

#[tokio::test]
async fn query_response_error_envelope() {
    let (app, session_id) = build_query_app().await;

    let json = post_query(&app, session_id, "MATCH (n:UnknownTag) RETURN n").await;

    assert_eq!(json["success"], false);
    assert_eq!(json["error"]["code"], "QUERY_ERROR");
    assert!(json["data"].is_null());
}
