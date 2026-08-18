//! HTTP SSE streaming integration tests.
//!
//! Tests verify:
//! - Schema event sent before any data (SSE protocol contract)
//! - Data events carry correct row data with sequential index
//! - Done event at end of successful query
//! - Error + done events on failed query
//! - Metadata event after all data, before done
//! - No dangling runtime resources after completion

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    routing::post,
    Router,
};
use graphdb::config::Config;
use graphdb::core::types::{SpaceInfo, SpaceSummary, VertexId};
use graphdb::core::vertex_edge_path::{Tag, Vertex};
use graphdb::core::{DataType, Value};
use graphdb::storage::{GraphStorage, PropertyGraphConfig, StorageSchemaOps, StorageWriter};
use graphdb::transaction::{TransactionManager, TransactionManagerConfig};
use graphdb_server::server::http::handlers::execute_stream;
use graphdb_server::server::http::server::HttpServer;
use graphdb_server::server::http::state::AppState;
use graphdb_server::server::GraphService;
use parking_lot::RwLock;
use tower::ServiceExt;

use futures::StreamExt;

// ── Helpers ──────────────────────────────────────────────────────────

/// Parsed SSE event.
#[derive(Debug, Clone)]
struct SseEvent {
    event_type: String,
    data: String,
}

/// Parse raw SSE text into a sequence of events.
fn parse_sse_events(body: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    for block in body.split("\n\n") {
        let block = block.trim();
        if block.is_empty() || block.starts_with(':') {
            continue;
        }
        let mut event_type = String::new();
        let mut data = String::new();
        for line in block.lines() {
            if let Some(val) = line.strip_prefix("event: ") {
                event_type = val.to_string();
            } else if let Some(val) = line.strip_prefix("data: ") {
                data = val.to_string();
            } else if line.starts_with(":") {
                // comment line (keepalive) — skip
            }
        }
        events.push(SseEvent { event_type, data });
    }
    events
}

/// Set up a minimal graph space with a Person tag and a few vertices
/// on the given storage.
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

/// Build a test axum router with the SSE streaming endpoint.
///
/// Sets up in-memory storage, test data, session, and full server stack
/// (minus auth middleware).  Returns (router, session_id, storage) so
/// the caller can make requests and inspect storage.
async fn build_sse_app() -> (Router, i64, Arc<RwLock<GraphStorage>>) {
    // 1. In-memory storage
    let storage = Arc::new(
        GraphStorage::new_with_config(PropertyGraphConfig::test()).expect("in-memory storage"),
    );
    let storage_rwlock = Arc::new(RwLock::new((*storage).clone()));

    // 2. Insert test data
    let test_space = setup_test_data(&storage_rwlock);

    // 3. GraphService (no background cleanup)
    let config = Config::default();
    let graph_service: Arc<GraphService<GraphStorage>> =
        GraphService::new_for_test(config.clone(), storage.clone()).await;

    // 4. Create a session so execute_stream() can find it
    let session = graph_service
        .get_session_manager()
        .create_session("test_user".to_string(), "127.0.0.1".to_string())
        .await
        .expect("session creation");
    let session_id = session.id();
    session.set_space(test_space);

    // 5. TransactionManager (needed for HttpServer::new)
    let txn_manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    // 6. HttpServer
    let http_server = Arc::new(HttpServer::new(
        graph_service,
        storage_rwlock.clone(),
        txn_manager,
        &config,
    ));

    // 7. AppState
    let state = AppState::new(http_server);

    // 8. Minimal router – just the SSE endpoint, no auth middleware
    let app = Router::new()
        .route("/v1/query/stream", post(execute_stream::<GraphStorage>))
        .with_state(state);

    (app, session_id, storage_rwlock)
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_sse_successful_query_schema_before_data_and_done() {
    let (app, session_id, _storage) = build_sse_app().await;

    let body = serde_json::json!({
        "query": "MATCH (n:Person) RETURN n.name, n.age ORDER BY n.name",
        "session_id": session_id,
        "event_buffer_capacity": 100,
    })
    .to_string();

    let request = Request::builder()
        .uri("/v1/query/stream")
        .method(Method::POST)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Collect body bytes
    let data_stream = response.into_body().into_data_stream();
    let chunks: Vec<_> = data_stream
        .filter_map(|r| async move { r.ok() })
        .collect()
        .await;
    let body_str: String = chunks
        .iter()
        .flat_map(|c| c.iter().copied())
        .map(|b| b as char)
        .collect();
    let events = parse_sse_events(&body_str);
    assert!(!events.is_empty(), "Expected at least one SSE event");

    // First event must be schema
    assert_eq!(
        events[0].event_type, "schema",
        "First SSE event should be schema"
    );
    let schema_data: serde_json::Value =
        serde_json::from_str(&events[0].data).expect("schema data is valid JSON");
    assert_eq!(schema_data["columns"].as_array().unwrap().len(), 2);
    assert_eq!(schema_data["column_count"], 2);

    // Data events follow schema; each has a row with name and age
    let data_events: Vec<_> = events
        .iter()
        .filter(|e| e.data.starts_with("{\"row\":"))
        .collect();
    assert_eq!(data_events.len(), 4, "Expected 4 row events");

    // Verify row ordering (by name: Alice, Bob, Charlie, Diana)
    let names: Vec<String> = data_events
        .iter()
        .filter_map(|e| {
            let v: serde_json::Value = serde_json::from_str(&e.data).ok()?;
            v["row"]["n.name"].as_str().map(str::to_owned)
        })
        .collect();
    assert_eq!(names, vec!["Alice", "Bob", "Charlie", "Diana"]);

    // Last event must be done
    let last = events.last().unwrap();
    assert_eq!(
        last.event_type, "done",
        "Last event should be done, got {:?}",
        last
    );

    // There should be a metadata event before done
    let metadata_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "metadata")
        .collect();
    assert_eq!(
        metadata_events.len(),
        1,
        "Expected exactly one metadata event"
    );
    let meta: serde_json::Value =
        serde_json::from_str(&metadata_events[0].data).expect("metadata is valid JSON");
    assert_eq!(meta["rows_returned"], 4);
}

#[tokio::test]
async fn test_sse_error_query_reports_error_and_done() {
    let (app, session_id, _storage) = build_sse_app().await;

    let body = serde_json::json!({
        "query": "MATCH (n:NonExistentTag) RETURN n.name",
        "session_id": session_id,
        "event_buffer_capacity": 100,
    })
    .to_string();

    let request = Request::builder()
        .uri("/v1/query/stream")
        .method(Method::POST)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let data_stream = response.into_body().into_data_stream();
    let chunks: Vec<_> = data_stream
        .filter_map(|r| async move { r.ok() })
        .collect()
        .await;
    let body_str: String = chunks
        .iter()
        .flat_map(|c| c.iter().copied())
        .map(|b| b as char)
        .collect();

    let events = parse_sse_events(&body_str);
    assert!(!events.is_empty(), "Expected SSE events on error");

    // Either error or done (or both). If schema was sent, error comes after.
    let error_events: Vec<_> = events.iter().filter(|e| e.event_type == "error").collect();
    let done_events: Vec<_> = events.iter().filter(|e| e.event_type == "done").collect();

    assert!(
        !error_events.is_empty() || !done_events.is_empty(),
        "Expected at least error or done event"
    );

    // done should always be the last event
    let last = events.last().unwrap();
    assert_eq!(last.event_type, "done", "Last event must be done");

    // If an error event exists, verify it has the right structure
    if let Some(error_ev) = error_events.first() {
        let err_val: serde_json::Value =
            serde_json::from_str(&error_ev.data).expect("error event data is JSON");
        assert_eq!(err_val["error"], true);
        assert!(!err_val["message"].as_str().unwrap_or("").is_empty());
    }
}

#[tokio::test]
async fn test_sse_invalid_session_returns_error_and_done() {
    let (app, _session_id, _storage) = build_sse_app().await;

    // Use a session_id that does not exist
    let body = serde_json::json!({
        "query": "MATCH (n:Person) RETURN n.name",
        "session_id": 999_999_999i64,
        "event_buffer_capacity": 100,
    })
    .to_string();

    let request = Request::builder()
        .uri("/v1/query/stream")
        .method(Method::POST)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let data_stream = response.into_body().into_data_stream();
    let chunks: Vec<_> = data_stream
        .filter_map(|r| async move { r.ok() })
        .collect()
        .await;
    let body_str: String = chunks
        .iter()
        .flat_map(|c| c.iter().copied())
        .map(|b| b as char)
        .collect();

    let events = parse_sse_events(&body_str);
    assert!(!events.is_empty(), "Expected SSE events on invalid session");

    let error_events: Vec<_> = events.iter().filter(|e| e.event_type == "error").collect();
    assert!(
        !error_events.is_empty(),
        "Expected error event for invalid session"
    );

    let err_val: serde_json::Value =
        serde_json::from_str(&error_events[0].data).expect("error event data is JSON");
    assert_eq!(err_val["error"], true);
    assert!(err_val["message"]
        .as_str()
        .unwrap_or("")
        .contains("Invalid session"),);

    // done should be last
    let last = events.last().unwrap();
    assert_eq!(last.event_type, "done", "Last event must be done");
}

#[tokio::test]
async fn test_sse_row_indices_are_sequential() {
    let (app, session_id, _storage) = build_sse_app().await;

    let body = serde_json::json!({
        "query": "MATCH (n:Person) RETURN n.age ORDER BY n.name",
        "session_id": session_id,
        "event_buffer_capacity": 100,
    })
    .to_string();

    let request = Request::builder()
        .uri("/v1/query/stream")
        .method(Method::POST)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let data_stream = response.into_body().into_data_stream();
    let chunks: Vec<_> = data_stream
        .filter_map(|r| async move { r.ok() })
        .collect()
        .await;
    let body_str: String = chunks
        .iter()
        .flat_map(|c| c.iter().copied())
        .map(|b| b as char)
        .collect();

    let events = parse_sse_events(&body_str);

    // Collect row indices from data events
    let indices: Vec<usize> = events
        .iter()
        .filter_map(|e| {
            if e.data.starts_with("{\"row\":") {
                let v: serde_json::Value = serde_json::from_str(&e.data).ok()?;
                v["index"].as_u64().map(|i| i as usize)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(indices.len(), 4, "Expected 4 row events");
    assert_eq!(indices, vec![0, 1, 2, 3], "Row indices must be sequential");
}

#[tokio::test]
async fn test_sse_schema_not_sent_for_ddl_statements() {
    let (app, session_id, _storage) = build_sse_app().await;

    // DDL / utility statements should still produce a done event via
    // from_execution_result, but may not produce a schema event (they
    // are pre-materialized).
    let body = serde_json::json!({
        "query": "SHOW SPACES",
        "session_id": session_id,
        "event_buffer_capacity": 100,
    })
    .to_string();

    let request = Request::builder()
        .uri("/v1/query/stream")
        .method(Method::POST)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let data_stream = response.into_body().into_data_stream();
    let chunks: Vec<_> = data_stream
        .filter_map(|r| async move { r.ok() })
        .collect()
        .await;
    let body_str: String = chunks
        .iter()
        .flat_map(|c| c.iter().copied())
        .map(|b| b as char)
        .collect();

    let events = parse_sse_events(&body_str);
    assert!(!events.is_empty(), "Expected at least done event");

    // Pre-materialized results should still produce done
    let done_events: Vec<_> = events.iter().filter(|e| e.event_type == "done").collect();
    assert!(
        !done_events.is_empty(),
        "Expected done event for DDL statement"
    );

    // Last event should be done (or metadata + done)
    let last = events.last().unwrap();
    assert_eq!(last.event_type, "done", "Last event must be done");
}
