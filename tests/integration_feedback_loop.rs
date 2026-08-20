//! Stats feedback loop end-to-end test.
//!
//! Verifies that after a query executes, estimated-vs-actual operator
//! feedback is recorded into the optimizer engine's shared
//! `QueryFeedbackHistory`.

use graphdb::core::stats::StatsManager;
use graphdb::core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Value, Vertex};
use graphdb::query::optimizer::OptimizerEngine;
use graphdb::query::QueryPipelineManager;
use graphdb::storage::{GraphStorage, StorageReader, StorageSchemaOps, StorageWriter};
use graphdb::test_utils::TestStorage;
use parking_lot::RwLock;
use std::sync::Arc;

const SPACE: &str = "feedback_test";

fn space_info(storage: &Arc<RwLock<GraphStorage>>) -> SpaceInfo {
    let id = storage.read().get_space_id(SPACE).expect("space id");
    let mut info = SpaceInfo::new(SPACE.to_string());
    info.space_id = id;
    info
}

fn setup_storage() -> Arc<RwLock<GraphStorage>> {
    let test_storage = TestStorage::new().expect("storage");
    let storage = test_storage.storage();
    {
        let mut storage = storage.write();
        let mut space = SpaceInfo::new(SPACE.to_string()).with_vid_type(DataType::BigInt);
        storage.create_space(&mut space).expect("create space");
        storage
            .create_tag(
                SPACE,
                &TagInfo::new("node".to_string()).with_properties(vec![
                    PropertyDef::new("value".to_string(), DataType::BigInt),
                    PropertyDef::new("name".to_string(), DataType::String),
                ]),
            )
            .expect("create tag");
        let vertices: Vec<Vertex> = (0..50)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        "node".to_string(),
                        vec![
                            ("value".to_string(), Value::BigInt(i)),
                            ("name".to_string(), Value::string(format!("node_{i}"))),
                        ]
                        .into_iter()
                        .collect(),
                    )],
                )
            })
            .collect();
        storage
            .batch_insert_vertices(SPACE, vertices)
            .expect("insert");
    }
    storage
}

#[test]
fn execution_feedback_is_recorded_after_query() {
    let storage = setup_storage();
    let optimizer_engine = Arc::new(OptimizerEngine::default());
    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        Arc::new(StatsManager::new()),
        optimizer_engine.clone(),
    );
    let space = space_info(&storage);

    // Execute a query that produces a real plan (scan -> project).
    let result = pipeline
        .execute_query_with_space("MATCH (n:node) RETURN n.value", Some(space.clone()))
        .expect("query should succeed");
    assert!(!result.to_data_set().expect("dataset").rows.is_empty());

    // The shared feedback history must now hold one entry for the query.
    let history = optimizer_engine.feedback_history();
    assert!(
        history.total_feedback_count() > 0,
        "feedback history must not be empty after execution"
    );
    let fingerprints = history.get_all_fingerprints();
    assert_eq!(fingerprints.len(), 1, "one fingerprint expected");

    // The recorded feedback must carry operator-level estimated vs actual rows.
    let feedbacks = history.get_feedback_for_query(&fingerprints[0]);
    assert_eq!(feedbacks.len(), 1);
    let feedback = &feedbacks[0];
    assert!(
        feedback.operator_feedback_count() > 0,
        "operator feedback must be recorded"
    );
    assert!(
        feedback
            .operator_feedbacks
            .iter()
            .any(|op| op.actual_rows > 0 && op.estimated_rows > 0),
        "scan operator must carry positive estimated and actual rows"
    );
}

#[test]
fn feedback_history_accumulates_across_queries() {
    let storage = setup_storage();
    let optimizer_engine = Arc::new(OptimizerEngine::default());
    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        Arc::new(StatsManager::new()),
        optimizer_engine.clone(),
    );
    let space = space_info(&storage);

    for _ in 0..3 {
        pipeline
            .execute_query_with_space("MATCH (n:node) RETURN n.value", Some(space.clone()))
            .expect("query should succeed");
    }

    let history = optimizer_engine.feedback_history();
    assert_eq!(history.query_count(), 1);
    assert_eq!(history.total_feedback_count(), 3);
}
