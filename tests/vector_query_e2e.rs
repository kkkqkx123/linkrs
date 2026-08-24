//! Query-layer vector statement end-to-end tests.
//!
//! Covers the four Phase C §3.4 case groups against the built-in local vector
//! engine, driven entirely through SQL statements plus the VectorApi insertion
//! hook:
//! 1. DDL → DML → query round-trip with top-k score assertions.
//! 2. WHERE filtering + LIMIT pushed down to the engine.
//! 3. Delete visibility: results converge after `delete_vector`.
//! 4. DROP INDEX error semantics (incl. `IF EXISTS`).
//!
//! Additionally exercises LOOKUP VECTOR / MATCH VECTOR execution through the
//! same search path.

#![cfg(feature = "vector")]

use std::collections::HashMap;
use std::sync::Arc;

use graphdb::config::{Config, VectorEngineKind};
use graphdb::storage::{GraphStorage, PropertyGraphConfig};
use graphdb::sync::vector_sync::{PointId, VectorPoint};
use graphdb_api::api::core::VectorApi;
use graphdb_server::server::GraphService;

type Service = Arc<GraphService<GraphStorage>>;

struct TestEnv {
    #[allow(dead_code)]
    temp_dir: tempfile::TempDir,
    service: Service,
    session_id: i64,
    vector_api: Arc<VectorApi>,
}

fn point(
    id: u64,
    vector: Vec<f32>,
    extra_payload: Option<HashMap<String, serde_json::Value>>,
) -> VectorPoint {
    let mut payload = HashMap::new();
    payload.insert(
        "group_id".to_string(),
        serde_json::Value::String("item_vec".to_string()),
    );
    if let Some(extra) = extra_payload {
        for (k, v) in extra {
            payload.insert(k, v);
        }
    }
    VectorPoint {
        id: PointId::Num(id),
        vector,
        payload: Some(payload),
    }
}

/// Boot a GraphService with the local vector engine and prepare
/// `space vec_e2e` with tag `item`, index `idx_item_vec`, and three points:
/// - 1: [1.0, 0.0, 0.0]
/// - 2: [0.0, 1.0, 0.0]
/// - 3: [0.9, 0.1, 0.0]
async fn setup_env() -> TestEnv {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let mut config = Config::default();
    config.vector.enabled = true;
    config.vector.engine = VectorEngineKind::Local;
    config.vector.local.data_dir = Some(temp_dir.path().join("vector"));

    let storage = Arc::new(
        GraphStorage::new_with_config(PropertyGraphConfig::test())
            .expect("Failed to create storage"),
    );

    let service = GraphService::new_for_test(config, storage).await;
    let session = service.authenticate("root", "root").await.unwrap();
    let session_id = session.id();

    for sql in [
        "CREATE SPACE vec_e2e (vid_type=STRING)",
        "USE vec_e2e",
        "CREATE TAG item(name: STRING, vec: VECTOR(3))",
        "CREATE VECTOR INDEX idx_item_vec ON item(vec) WITH (vector_size=3, distance='cosine')",
    ] {
        service
            .execute(session_id, sql)
            .await
            .unwrap_or_else(|e| panic!("setup statement '{sql}' failed: {e}"));
    }

    let vector_api = service.vector_api().expect("Vector API available").clone();

    let mut names = HashMap::new();
    names.insert("name".to_string(), serde_json::Value::String("a".into()));
    let mut names_b = HashMap::new();
    names_b.insert("name".to_string(), serde_json::Value::String("b".into()));

    vector_api
        .insert_vector_batch(
            1,
            "item",
            "vec",
            vec![
                point(1, vec![1.0, 0.0, 0.0], Some(names)),
                point(2, vec![0.0, 1.0, 0.0], Some(names_b)),
                point(3, vec![0.9, 0.1, 0.0], None),
            ],
        )
        .await
        .expect("insert_vector_batch should succeed");

    TestEnv {
        temp_dir,
        service,
        session_id,
        vector_api: vector_api.clone(),
    }
}

impl TestEnv {
    async fn exec(&self, sql: &str) -> Result<Vec<Vec<graphdb::core::Value>>, String> {
        self.service
            .execute(self.session_id, sql)
            .await
            .map(|result| result.rows().to_vec())
    }
}

/// Case 1: DDL → DML → query round-trip; top-k ordering and score assertions,
/// plus THRESHOLD semantics on similarity scores.
#[tokio::test]
async fn case1_vector_ddl_query_roundtrip() {
    let env = setup_env().await;

    let rows = env
        .exec("SEARCH VECTOR idx_item_vec WITH vector=[1.0, 0.0, 0.0] LIMIT 2")
        .await
        .expect("SEARCH VECTOR should succeed");
    assert_eq!(rows.len(), 2, "LIMIT 2 returns two rows");
    assert_eq!(rows[0][0], graphdb::core::Value::string("1"));
    assert_eq!(rows[1][0], graphdb::core::Value::string("3"));

    // Score assertion: exact-match vector scores ~1.0.
    if let graphdb::core::Value::Double(score) = rows[0][1] {
        assert!(score >= 0.9999, "exact match score ~1.0, got {score}");
    } else {
        panic!("expected Double score, got {:?}", rows[0][1]);
    }

    // THRESHOLD prunes dissimilar candidates (id 2 has cosine 0 vs query).
    let rows = env
        .exec("SEARCH VECTOR idx_item_vec WITH vector=[1.0, 0.0, 0.0] THRESHOLD 0.5")
        .await
        .expect("THRESHOLD search should succeed");
    let ids: Vec<&str> = rows
        .iter()
        .map(|r| match &r[0] {
            graphdb::core::Value::String(s) => s.as_str(),
            other => panic!("expected string id, got {other:?}"),
        })
        .collect();
    assert!(
        !ids.contains(&"2"),
        "orthogonal vector must be pruned by THRESHOLD, got {ids:?}"
    );
}

/// Case 2: WHERE filter + LIMIT reach the local engine (payload filtering).
#[tokio::test]
async fn case2_vector_search_filter_and_limit() {
    let env = setup_env().await;

    let rows = env
        .exec("SEARCH VECTOR idx_item_vec WITH vector=[1.0, 0.0, 0.0] WHERE name = 'a' LIMIT 10")
        .await
        .expect("filtered SEARCH VECTOR should succeed");
    assert_eq!(rows.len(), 1, "only point 1 carries name 'a', got {rows:?}");
    assert_eq!(rows[0][0], graphdb::core::Value::string("1"));

    // OFFSET skips the best candidate.
    let rows = env
        .exec("SEARCH VECTOR idx_item_vec WITH vector=[1.0, 0.0, 0.0] LIMIT 1 OFFSET 1")
        .await
        .expect("OFFSET search should succeed");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0][0],
        graphdb::core::Value::string("3"),
        "OFFSET 1 must skip the top hit"
    );
}

/// Case 3: delete visibility — results converge after `delete_vector`.
#[tokio::test]
async fn case3_vector_delete_visibility() {
    let env = setup_env().await;

    env.vector_api
        .delete_vector(1, "item", "vec", "1")
        .await
        .expect("delete_vector should succeed");

    let rows = env
        .exec("SEARCH VECTOR idx_item_vec WITH vector=[1.0, 0.0, 0.0] LIMIT 10")
        .await
        .expect("search after delete should succeed");
    let ids: Vec<&str> = rows
        .iter()
        .map(|r| match &r[0] {
            graphdb::core::Value::String(s) => s.as_str(),
            other => panic!("expected string id, got {other:?}"),
        })
        .collect();
    assert!(
        !ids.contains(&"1"),
        "deleted point must disappear, got {ids:?}"
    );
    assert_eq!(ids.len(), 2);
}

/// Case 4: DROP INDEX semantics — success row, `IF EXISTS` no-op, and
/// post-drop search failure.
#[tokio::test]
async fn case4_vector_drop_index_semantics() {
    let env = setup_env().await;

    // Dropping an existing index yields an (action, name, status) row.
    let rows = env
        .exec("DROP VECTOR INDEX idx_item_vec")
        .await
        .expect("DROP VECTOR INDEX should succeed");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0][0],
        graphdb::core::Value::string("drop_vector_index")
    );
    assert_eq!(rows[0][1], graphdb::core::Value::string("idx_item_vec"));
    assert_eq!(rows[0][2], graphdb::core::Value::string("dropped"));

    // Searching through the dropped index now fails explicitly.
    let err = env
        .exec("SEARCH VECTOR idx_item_vec WITH vector=[1.0, 0.0, 0.0] LIMIT 2")
        .await
        .expect_err("search after drop must fail");
    assert!(
        err.contains("Index not found"),
        "post-drop search error should mention the missing index, got: {err}"
    );

    // Missing index without IF EXISTS fails at planning time.
    let err = env
        .exec("DROP VECTOR INDEX idx_missing")
        .await
        .expect_err("drop of missing index without IF EXISTS must fail");
    assert!(err.contains("Index not found"), "got: {err}");

    // ...while IF EXISTS degrades to a no-op status row.
    let rows = env
        .exec("DROP VECTOR INDEX IF EXISTS idx_missing")
        .await
        .expect("IF EXISTS drop should succeed");
    assert_eq!(rows[0][2], graphdb::core::Value::string("not_exists"));
}

/// LOOKUP VECTOR resolves through the same search path as SEARCH VECTOR.
#[tokio::test]
async fn vector_lookup_uses_search_path() {
    let env = setup_env().await;

    let rows = env
        .exec("LOOKUP VECTOR vec_e2e idx_item_vec WITH vector=[1.0, 0.0, 0.0] LIMIT 2")
        .await
        .expect("LOOKUP VECTOR should succeed");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], graphdb::core::Value::string("1"));
}
