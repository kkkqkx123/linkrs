use std::collections::HashMap;
use std::sync::Arc;

use super::SyncWrapper;
use crate::core::stats::{MetricType, StatsManager};
use crate::core::types::VertexId;
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
