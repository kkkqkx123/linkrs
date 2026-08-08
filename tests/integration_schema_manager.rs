//! Schema Manager Initialization Integration Tests
//!
//! Test coverage:
//! - Schema manager is properly initialized when vector search is disabled
//! - Schema manager is properly initialized when vector search is enabled but fails
//! - Basic DDL operations work regardless of vector search configuration

use graphdb::core::stats::StatsManager;
use graphdb::query::optimizer::OptimizerEngine;
use graphdb::query::QueryPipelineManager;
use graphdb::test_utils::TestStorage;
use std::sync::Arc;

/// Test that QueryPipelineManager works without schema_manager
/// This simulates the scenario where vector search is enabled but fails to
/// initialize. Space creation lives at the storage layer, so storage-level
/// DDL still works without a schema manager; session space tracking is
/// schema-manager driven, so space-scoped statements fail with a clear error.
#[test]
fn test_pipeline_manager_without_schema_manager() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    // Without schema_manager, CREATE SPACE still works: space ownership is
    // storage-level and does not require the metadata manager.
    let result = pipeline_manager.execute_query("CREATE SPACE test_space (vid_type=STRING)");
    assert!(
        result.is_ok(),
        "CREATE SPACE should succeed without schema_manager: {:?}",
        result.err()
    );

    // The no-schema-manager symptom is session space tracking: USE does not
    // record a current space, so space-scoped statements fail with a clear
    // error instead of silently targeting the wrong space.
    let use_result = pipeline_manager.execute_query("USE test_space");
    assert!(
        use_result.is_ok(),
        "USE should succeed without schema_manager: {:?}",
        use_result.err()
    );
    let result = pipeline_manager.execute_query("MATCH (n:person) RETURN n");
    assert!(
        result.is_err(),
        "space-scoped query should fail without schema_manager"
    );
    let error_msg = format!("{:?}", result.err()).to_lowercase();
    assert!(
        error_msg.contains("space") && error_msg.contains("does not exist"),
        "Error should clearly mention the missing space: {}",
        error_msg
    );
}

/// Test that QueryPipelineManager works with schema_manager
#[test]
fn test_pipeline_manager_with_schema_manager() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());
    let schema_manager = test_storage.schema_manager();

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage.clone(),
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager);

    // With schema_manager, CREATE SPACE should work
    let result = pipeline_manager.execute_query("CREATE SPACE test_space2 (vid_type=STRING)");
    assert!(
        result.is_ok(),
        "CREATE SPACE should succeed with schema_manager: {:?}",
        result.err()
    );
}

/// Test basic operations work when schema_manager is provided
#[test]
fn test_basic_ddl_with_schema_manager() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());
    let schema_manager = test_storage.schema_manager();

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage.clone(),
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager);

    // Create space
    let result = pipeline_manager.execute_query("CREATE SPACE test_ddl (vid_type=STRING)");
    assert!(
        result.is_ok(),
        "CREATE SPACE should succeed: {:?}",
        result.err()
    );

    // Use space - this sets the current space in session
    let result = pipeline_manager.execute_query("USE test_ddl");
    assert!(result.is_ok(), "USE should succeed: {:?}", result.err());

    // Note: CREATE TAG requires the space to be selected in the session context
    // This test verifies that schema_manager is properly initialized
    // The actual CREATE TAG may require additional session setup
}

/// Test that error messages are clear when schema_manager is missing
#[test]
fn test_error_message_clarity_without_schema_manager() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    // Storage-level DDL (CREATE SPACE) remains available without a schema
    // manager; the failure surface is space-scoped statements after USE,
    // whose error must clearly name the missing space.
    let result = pipeline_manager.execute_query("CREATE SPACE test (vid_type=STRING)");
    assert!(
        result.is_ok(),
        "CREATE SPACE should succeed without schema_manager: {:?}",
        result.err()
    );
    let use_result = pipeline_manager.execute_query("USE test");
    assert!(
        use_result.is_ok(),
        "USE should succeed without schema_manager: {:?}",
        use_result.err()
    );
    let result = pipeline_manager.execute_query("MATCH (n:person) RETURN n");
    assert!(
        result.is_err(),
        "space-scoped query should fail without schema_manager"
    );
    let error_msg = format!("{:?}", result.err()).to_lowercase();
    assert!(
        error_msg.contains("space") && error_msg.contains("does not exist"),
        "Error should clearly name the missing space: {}",
        error_msg
    );

    // Note: Other operations like CREATE TAG may fail with different errors
    // because they resolve names through the schema manager, which is
    // expected behavior in the degraded mode.
}
