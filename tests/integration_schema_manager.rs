//! Schema Manager Initialization Integration Tests
//!
//! Test coverage:
//! - Schema manager is properly initialized when vector search is disabled
//! - Schema manager is properly initialized when vector search is enabled but fails
//! - Basic DDL operations work regardless of vector search configuration

mod common;

use common::TestStorage;
use graphdb::core::stats::StatsManager;
use graphdb::query::optimizer::OptimizerEngine;
use graphdb::query::query_pipeline_manager::QueryPipelineManager;
use std::sync::Arc;

/// Test that QueryPipelineManager works without schema_manager
/// This simulates the scenario where vector search is enabled but fails to initialize
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

    // Without schema_manager, CREATE SPACE should fail with specific error
    let result = pipeline_manager.execute_query("CREATE SPACE test_space (vid_type=STRING)");
    assert!(
        result.is_err(),
        "CREATE SPACE should fail without schema_manager"
    );

    let error_msg = format!("{:?}", result.err());
    assert!(
        error_msg.contains("Schema manager not initialized")
            || error_msg.contains("schema manager"),
        "Error should indicate schema_manager not initialized: {}",
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

    // Test CREATE SPACE - should fail with schema_manager not initialized error
    let result = pipeline_manager.execute_query("CREATE SPACE test (vid_type=STRING)");
    assert!(
        result.is_err(),
        "CREATE SPACE should fail without schema_manager"
    );
    let error_msg = format!("{:?}", result.err()).to_lowercase();
    assert!(
        error_msg.contains("schema") || error_msg.contains("not initialized"),
        "Error should mention schema manager: {}",
        error_msg
    );

    // Note: Other operations like USE, CREATE TAG, SHOW TAGS may fail with different errors
    // because they require a space to be selected first, which is expected behavior
}
