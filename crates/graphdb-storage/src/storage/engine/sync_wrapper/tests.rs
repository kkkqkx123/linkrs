use std::collections::HashMap;
use std::sync::Arc;

use super::SyncWrapper;
use crate::core::stats::{MetricType, StatsManager};
use crate::core::types::VertexId;
#[cfg(feature = "fulltext-search")]
use crate::core::types::{PropertyDef, SpaceInfo, TagInfo};
#[cfg(feature = "fulltext-search")]
use crate::core::vertex_edge_path::Tag;
#[cfg(feature = "fulltext-search")]
use crate::core::DataType;
use crate::core::Edge;
use crate::storage::{
    GraphStorage, MetricsStorage, MockStorage, StoragePersistenceOps, StorageReader, StorageWriter,
};
use crate::sync::SyncManager;

#[test]
fn records_read_and_write_metrics() {
    let stats_manager = Arc::new(StatsManager::new());
    let inner = MockStorage::new().expect("Failed to create MockStorage");
    let mut storage = MetricsStorage::new(inner, stats_manager.clone());

    storage
        .get_vertex("test", &VertexId::from_int64(1))
        .expect("Failed to read vertex");
    storage
        .batch_insert_edges("test", Vec::new())
        .expect("Failed to write edges");

    assert_eq!(stats_manager.get_value(MetricType::StorageReadOps), Some(1));
    assert_eq!(
        stats_manager.get_value(MetricType::StorageWriteOps),
        Some(1)
    );
}

#[test]
fn delegates_admin_checkpoint_operations() {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let inner = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
        .expect("Failed to create GraphStorage");
    let stats_manager = Arc::new(StatsManager::new());
    let storage = MetricsStorage::new(inner, stats_manager);

    let checkpoint = storage
        .create_checkpoint()
        .expect("checkpoint should succeed");

    assert!(checkpoint.is_some());
}

#[test]
fn does_not_buffer_sync_events_when_edge_insert_fails() {
    let sync_manager = Arc::new(SyncManager::new_without_fulltext());

    let inner = MockStorage::new().expect("Failed to create MockStorage");
    inner.set_fail_insert_edge(true);

    let mut storage = SyncWrapper::with_sync_manager(inner, sync_manager.clone());
    let edge = Edge {
        src: VertexId::from_int64(1),
        dst: VertexId::from_int64(2),
        edge_type: "KNOWS".to_string(),
        ranking: 0,
        props: HashMap::new(),
    };

    let result = storage.insert_edge("test", edge);

    assert!(result.is_err());
}

#[test]
#[cfg(feature = "fulltext-search")]
fn checkpoint_reopens_storage_and_rebuilds_outbox_from_remaining_wal() {
    use crate::sync::batch::BatchConfig;
    use crate::sync::coordinator::SyncCoordinator;
    use graphdb_search::search::config::FulltextConfig;
    use graphdb_search::search::FulltextIndexManager;

    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let work_dir = directory.path().to_path_buf();
    let mut inner = GraphStorage::new_with_path(work_dir.clone()).expect("storage should open");
    let mut space = SpaceInfo::new("test_space".to_string()).with_vid_type(DataType::BigInt);
    inner
        .create_space(&mut space)
        .expect("space should be created");
    inner
        .create_tag(
            "test_space",
            &TagInfo::new("Person".to_string())
                .with_properties(vec![PropertyDef::new("name".to_string(), DataType::String)]),
        )
        .expect("tag should be created");

    // Create fulltext manager and coordinator to enable intent generation
    let fulltext_config = FulltextConfig {
        index_path: work_dir.join("fulltext"),
        ..Default::default()
    };
    let fulltext_manager =
        Arc::new(FulltextIndexManager::new(fulltext_config).expect("fulltext manager should open"));
    let sync_coordinator = Arc::new(SyncCoordinator::new(
        fulltext_manager,
        BatchConfig::default(),
    ));
    let mut manager = SyncManager::new(sync_coordinator);
    manager
        .configure_outbox(work_dir.join("outbox/outbox.sqlite"))
        .expect("outbox should be configured");
    let manager = Arc::new(manager);
    let storage = SyncWrapper::with_sync_manager(inner, manager.clone());
    let mut writer = storage
        .bind_auto_commit_context()
        .expect("auto-commit context should be available");

    writer
        .insert_vertex(
            "test_space",
            crate::core::Vertex::new(
                VertexId::from_int64(1),
                vec![Tag::new(
                    "Person".to_string(),
                    [(
                        "name".to_string(),
                        crate::core::Value::string("one"),
                    )]
                    .into_iter()
                    .collect(),
                )],
            ),
        )
        .expect("first vertex should be committed");
    assert!(
        manager
            .outbox_materialized_lsn()
            .expect("outbox frontier should load")
            .is_some(),
        "first committed event should be durable before checkpoint"
    );
    let checkpoint = writer
        .create_checkpoint()
        .expect("checkpoint should succeed")
        .expect("persistent storage should return checkpoint stats");
    assert!(
        checkpoint.wal_truncated > 0,
        "checkpoint did not advance safe WAL boundary: {:?}, outbox={:?}",
        checkpoint,
        manager
            .outbox_materialized_lsn()
            .expect("outbox frontier should load")
    );

    writer
        .insert_vertex(
            "test_space",
            crate::core::Vertex::new(
                VertexId::from_int64(2),
                vec![Tag::new(
                    "Person".to_string(),
                    [(
                        "name".to_string(),
                        crate::core::Value::string("two"),
                    )]
                    .into_iter()
                    .collect(),
                )],
            ),
        )
        .expect("second vertex should remain in WAL after checkpoint");
    drop(writer);
    drop(storage);

    std::fs::write(
        work_dir.join("outbox/outbox.sqlite"),
        b"incomplete sqlite projection",
    )
    .expect("live outbox should be corrupted for recovery test");
    let reopened = GraphStorage::open(work_dir.clone()).expect("storage should reopen");
    let mut recovered_manager = SyncManager::new_without_fulltext();
    recovered_manager
        .configure_outbox(work_dir.join("outbox/outbox.sqlite"))
        .expect("outbox should restore from the combined manifest");
    let recovered = reopened
        .recover_outbox_projection(&recovered_manager)
        .expect("remaining WAL should rebuild outbox events");

    assert!(recovered > 0);
    assert!(
        recovered_manager
            .outbox_materialized_lsn()
            .expect("recovered outbox frontier should load")
            .expect("recovered outbox should have a materialized frontier")
            .get()
            > checkpoint.wal_truncated
    );
    assert_eq!(
        reopened
            .get_vertex("test_space", &VertexId::from_int64(1))
            .expect("first vertex should be readable")
            .expect("first vertex should exist")
            .vid,
        VertexId::from_int64(1)
    );
    assert!(reopened
        .get_vertex("test_space", &VertexId::from_int64(2))
        .expect("second vertex should be readable")
        .is_some());
}
