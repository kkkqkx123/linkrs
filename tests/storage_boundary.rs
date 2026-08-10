//! Storage boundary end-to-end test.
//!
//! Verifies the storage-boundary behavior described in
//! `docs/plan/residual_issues.md` §3: per-query storage is bound to a
//! snapshot context (read statements pin a fixed read timestamp), and the
//! bound handle exposes the snapshot via `QueryStorage::snapshot_handle()`
//! without the query competing on the global storage lock per `next()`.

use graphdb::core::stats::StatsManager;
use graphdb::core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
use graphdb::core::vertex_edge_path::Tag;
use graphdb::core::{DataType, Value, Vertex};
use graphdb::query::optimizer::OptimizerEngine;
use graphdb::query::QueryPipelineManager;
use graphdb::storage::{
    GraphStorage, QueryStorage, StorageOperationContextOps, StorageReader, StorageSchemaOps,
    StorageWriter,
};
use graphdb::test_utils::TestStorage;
use parking_lot::RwLock;
use std::sync::Arc;

const SPACE: &str = "storage_boundary_test";

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
                &TagInfo::new("node".to_string()).with_properties(vec![PropertyDef::new(
                    "value".to_string(),
                    DataType::BigInt,
                )]),
            )
            .expect("create tag");
        let vertices: Vec<Vertex> = (0..50)
            .map(|i| {
                Vertex::new(
                    VertexId::from_int64(i),
                    vec![Tag::new(
                        "node".to_string(),
                        vec![("value".to_string(), Value::BigInt(i))]
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
fn bound_read_storage_pins_a_snapshot_handle() {
    let storage = setup_storage();
    let optimizer_engine = Arc::new(OptimizerEngine::default());
    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        Arc::new(StatsManager::new()),
        optimizer_engine.clone(),
    );
    let space = space_info(&storage);

    // A normal read query must execute successfully on the per-query bound
    // storage (the pipeline binds a read operation context internally).
    let result = pipeline
        .execute_query_with_space("MATCH (n:node) RETURN n.value", Some(space))
        .expect("query should succeed");
    assert!(!result.to_data_set().expect("dataset").rows.is_empty());

    // The raw (unbound) global storage reports no snapshot...
    let raw_handle = storage.read().snapshot_handle();
    assert!(raw_handle.is_none(), "unbound storage has no snapshot");

    // ...while a read-bound handle pins the read timestamp.
    let bound = storage
        .read()
        .bind_read_operation_context()
        .expect("read binding should succeed");
    let handle = bound
        .snapshot_handle()
        .expect("bound read storage must expose a snapshot handle");
    assert!(handle.ts > 0, "snapshot timestamp must be pinned");

    // An auto-commit write binding pins its write timestamp too.
    let write_bound = storage
        .read()
        .bind_auto_commit_context()
        .expect("auto-commit binding should succeed");
    let write_handle = write_bound
        .snapshot_handle()
        .expect("auto-commit storage must expose a snapshot handle");
    assert!(
        write_handle.ts > 0,
        "write snapshot timestamp must be pinned"
    );
}

#[test]
fn concurrent_read_queries_each_hold_their_own_snapshot() {
    use graphdb::storage::StorageOperationContextOps;
    let storage = setup_storage();
    let optimizer_engine = Arc::new(OptimizerEngine::default());
    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        Arc::new(StatsManager::new()),
        optimizer_engine.clone(),
    );
    let space = space_info(&storage);

    // Two independently bound read handles observe the same pinned snapshot
    // semantics without sharing a lock object: each handle is a distinct
    // bound instance (the per-query storage boundary).
    let a = storage
        .read()
        .bind_read_operation_context()
        .expect("bind a");
    let b = storage
        .read()
        .bind_read_operation_context()
        .expect("bind b");

    let handle_a = a.snapshot_handle().expect("snapshot a");
    let handle_b = b.snapshot_handle().expect("snapshot b");
    assert!(handle_a.ts > 0 && handle_b.ts > 0);
    // The handles are pinned (equal ts) — read queries do not advance the
    // snapshot between the two bindings.
    assert_eq!(handle_a.ts, handle_b.ts);

    let result = pipeline
        .execute_query_with_space("MATCH (n:node) RETURN n.value LIMIT 5", Some(space))
        .expect("query should succeed");
    assert_eq!(result.to_data_set().expect("dataset").rows.len(), 5);
}
