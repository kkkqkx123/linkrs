//! Cache Invalidation Tests

use graphdb::query::cache::{
    DataChangeEvent, DataChangeType, DependencyTracker, InvalidationManager, InvalidationStats,
    TableBasedInvalidation,
};

#[test]
fn test_data_change_event_creation() {
    let event = DataChangeEvent::insert("users");

    assert_eq!(event.table_name, "users");
    assert_eq!(event.change_type, DataChangeType::Insert);
}

#[test]
fn test_data_change_type_variants() {
    assert_eq!(DataChangeType::Insert as i32, 0);
    assert_eq!(DataChangeType::Update as i32, 1);
    assert_eq!(DataChangeType::Delete as i32, 2);
    assert_eq!(DataChangeType::SchemaChange as i32, 3);
    assert_eq!(DataChangeType::BulkLoad as i32, 4);
    assert_eq!(DataChangeType::Truncate as i32, 5);
}

#[test]
fn test_dependency_tracker() {
    let tracker = DependencyTracker::new();

    tracker.register_dependency("query1".to_string(), "users".to_string());
    tracker.register_dependency("query1".to_string(), "posts".to_string());
    tracker.register_dependency("query2".to_string(), "users".to_string());

    let deps = tracker.get_dependencies("query1");
    assert_eq!(deps.len(), 2);
    assert!(deps.contains("users"));
    assert!(deps.contains("posts"));
}

#[test]
fn test_dependency_tracker_get_dependent_keys() {
    let tracker = DependencyTracker::new();

    tracker.register_dependency("query1".to_string(), "users".to_string());
    tracker.register_dependency("query2".to_string(), "users".to_string());

    let keys = tracker.get_dependent_keys("users");
    assert_eq!(keys.len(), 2);
    assert!(keys.contains("query1"));
    assert!(keys.contains("query2"));
}

#[test]
fn test_table_based_invalidation() {
    let invalidation = TableBasedInvalidation::new();

    invalidation.register_dependency("query1".to_string(), "users".to_string());

    let event = DataChangeEvent::update("users");
    let keys = invalidation.get_keys_to_invalidate(&event);

    assert!(keys.contains("query1"));
}

#[test]
fn test_invalidation_manager() {
    let manager = InvalidationManager::new();

    manager.register_dependency("query1".to_string(), "users".to_string());

    let event = DataChangeEvent::update("users");
    manager.on_data_change(&event);

    let stats = manager.stats();
    assert!(stats.total_invalidations > 0);
}

#[test]
fn test_invalidation_stats() {
    let stats = InvalidationStats {
        total_invalidations: 10,
        table_invalidations: 8,
        full_invalidations: 2,
    };

    assert_eq!(stats.total_invalidations, 10);
    assert_eq!(stats.table_invalidations, 8);
    assert_eq!(stats.full_invalidations, 2);
}
