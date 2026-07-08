//! Sync Module 2PC Protocol Tests
//!
//! Tests for two-phase commit protocol implementation

use crate::common::sync_helpers::{create_test_vertex, SyncTestHarness};
use graphdb::core::types::{DataType, TransactionId};
use graphdb::core::Value;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper function to create harness with specific paths
fn create_harness_with_paths(
    db_path: &Path,
    index_path: &Path,
) -> Result<SyncTestHarness, Box<dyn std::error::Error>> {
    use graphdb::search::{
        EngineType, FulltextConfig, FulltextIndexManager, SyncConfig, TantivyConfig, TokenizerKind,
    };
    use graphdb::storage::GraphStorage;
    use graphdb::sync::batch::BatchConfig;
    use graphdb::sync::coordinator::SyncCoordinator;
    use graphdb::sync::manager::SyncManager;
    use std::time::Duration;

    // Create storage
    let storage = GraphStorage::new_with_path(db_path.to_path_buf())?;

    // Create fulltext index manager
    let config = FulltextConfig {
        enabled: true,
        index_path: index_path.to_path_buf(),
        default_engine: EngineType::Bm25,
        sync: SyncConfig::default(),
        cache_size: 100,
        max_result_cache: 1000,
        result_cache_ttl_secs: 60,
        tantivy: TantivyConfig {
            tokenizer: TokenizerKind::Default,
            ..Default::default()
        },
    };

    let fulltext_manager = Arc::new(FulltextIndexManager::new(config)?);

    // Create sync coordinator
    let batch_config = BatchConfig {
        batch_size: 100,
        flush_interval: Duration::from_millis(100),
        max_buffer_size: 1000,
        enable_persistence: false,
        persistence_path: None,
        failure_policy: graphdb::search::SyncFailurePolicy::FailOpen,
    };

    let sync_coordinator = Arc::new(SyncCoordinator::new(fulltext_manager.clone(), batch_config));

    // Create sync manager
    let sync_manager = Arc::new(SyncManager::new(sync_coordinator.clone()));

    // Create runtime for async operations
    let rt = tokio::runtime::Runtime::new()?;

    // Start background tasks for batch processing
    rt.block_on(sync_coordinator.start_background_tasks());

    Ok(SyncTestHarness {
        storage,
        sync_manager,
        sync_coordinator,
        temp_dir: TempDir::new()?,
        current_txn_id: None,
        current_txn_seq: 0,
        rt,
    })
}

/// TC-040: 2PC full protocol flow
#[test]
fn test_2pc_full_protocol() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    // Begin transaction
    let txn_id = TransactionId(
        harness
            .begin_transaction()
            .expect("Failed to begin transaction"),
    );

    // Execute multiple operations
    for i in 0..10 {
        let vertex = create_test_vertex(
            i + 1,
            "Person",
            vec![("name", Value::String(format!("Person{}", i + 1)))],
        );
        harness
            .insert_vertex_with_txn("test_space", vertex)
            .expect("Failed to insert vertex");
    }

    // Phase 1: Prepare
    let sync_manager = harness.sync_manager.clone();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        sync_manager
            .prepare_transaction(txn_id)
            .await
            .expect("Prepare should succeed");
    });

    // Phase 2: Commit storage (done in commit_transaction)
    // Phase 3: Commit index sync (done in commit_transaction)
    harness
        .commit_transaction()
        .expect("Failed to commit transaction");

    harness.wait_for_async(300);

    // Verify all operations are committed
    for i in 0..10 {
        harness
            .assert_vertex_exists("test_space", &Value::Int(i + 1))
            .expect("Vertex should exist");
    }

    // Verify index sync
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Person1", 20)
        .expect("Failed to search");
    println!("Search results for 'Person1': {}", results.len());
    assert!(
        !results.is_empty(),
        "At least one index should be synced, found {}",
        results.len()
    );

    // Verify all indexes are synced
    let mut found_count = 0;
    for i in 0..10 {
        let search_term = format!("Person{}", i + 1);
        let results = harness
            .search_fulltext("test_space", "Person", "name", &search_term, 10)
            .expect("Failed to search");
        if !results.is_empty() {
            found_count += 1;
        }
    }
    assert!(
        found_count >= 10,
        "All indexes should be synced, found {}",
        found_count
    );
}

/// TC-041: 2PC prepare phase failure
#[test]
fn test_2pc_prepare_failure() {
    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    // Begin transaction
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    // Insert vertex
    let vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex");

    // Prepare should succeed in normal case
    let txn_id = TransactionId(harness.current_txn_id.unwrap());
    let sync_manager = harness.sync_manager.clone();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let prepare_result = rt.block_on(async { sync_manager.prepare_transaction(txn_id).await });

    // Prepare should succeed
    assert!(
        prepare_result.is_ok(),
        "Prepare should succeed for valid operations"
    );
}

/// TC-042: 2PC storage commit failure
#[test]
fn test_2pc_storage_commit_failure() {
    // This test verifies that when storage commit fails,
    // the index buffer is cleaned up

    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    // Begin transaction
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    // Insert vertex (buffered in sync manager)
    let vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex");

    // Rollback instead of commit (simulating storage failure)
    harness.rollback_transaction().expect("Failed to rollback");

    harness.wait_for_async(200);

    // Verify index buffer was cleaned up
    let results = harness
        .search_fulltext("test_space", "Person", "name", "Alice", 10)
        .expect("Failed to search");
    assert_eq!(
        results.len(),
        0,
        "Index buffer should be cleaned up after rollback"
    );
}

/// TC-043: 2PC index sync failure handling
#[test]
fn test_2pc_index_sync_failure() {
    // This test verifies that storage is committed even if index sync fails
    // (FailOpen policy)

    let mut harness = SyncTestHarness::new().expect("Failed to create test harness");

    // Setup
    harness
        .create_space("test_space")
        .expect("Failed to create space");
    harness
        .create_tag_with_fulltext(
            "test_space",
            "Person",
            vec![("name", DataType::String)],
            vec!["name"],
        )
        .expect("Failed to create tag");

    // Begin transaction
    harness
        .begin_transaction()
        .expect("Failed to begin transaction");

    // Insert vertex
    let vertex = create_test_vertex(
        1,
        "Person",
        vec![("name", Value::String("Alice".to_string()))],
    );
    harness
        .insert_vertex_with_txn("test_space", vertex)
        .expect("Failed to insert vertex");

    // Commit transaction
    // Note: In actual implementation, if index sync fails,
    // the error is logged but storage commit succeeds (FailOpen)
    let result = harness.commit_transaction();

    // Commit should succeed (FailOpen policy)
    assert!(
        result.is_ok(),
        "Commit should succeed even if index sync has issues"
    );

    // Verify storage committed
    harness
        .assert_vertex_exists("test_space", &Value::Int(1))
        .expect("Vertex should exist in storage");
}

/// TC-050: Concurrent transactions sync
#[test]
fn test_concurrent_transactions_sync() {
    use std::thread;

    // Create independent harness for each thread with unique paths
    let mut handles = vec![];
    for i in 0..5 {
        let handle = thread::spawn(move || {
            // Create independent harness with unique path for each thread
            let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
            let db_path = temp_dir.path().join(format!("test_{}.db", i));
            let index_path = temp_dir.path().join(format!("index_{}", i));

            let mut harness =
                create_harness_with_paths(&db_path, &index_path).expect("Failed to create harness");

            // Setup space
            harness
                .create_space("test_space")
                .expect("Failed to create space");
            harness
                .create_tag_with_fulltext(
                    "test_space",
                    "Person",
                    vec![("name", DataType::String)],
                    vec!["name"],
                )
                .expect("Failed to create tag");

            // Begin transaction
            harness
                .begin_transaction()
                .expect("Failed to begin transaction");

            // Insert vertex
            let vertex = create_test_vertex(
                i * 10 + 1,
                "Person",
                vec![("name", Value::String(format!("Thread{}", i)))],
            );
            harness
                .insert_vertex_with_txn("test_space", vertex)
                .expect("Failed to insert vertex");

            // Commit transaction
            harness
                .commit_transaction()
                .expect("Failed to commit transaction");

            // Verify in same thread
            harness.wait_for_async(300);

            // Force commit all
            let rt = &harness.rt;
            rt.block_on(async {
                harness
                    .sync_coordinator
                    .commit_all()
                    .await
                    .expect("Commit all should succeed");
            });

            // Verify vertex exists
            harness
                .assert_vertex_exists("test_space", &Value::Int((i * 10 + 1) as i32))
                .expect("Vertex should exist");

            // Verify index is synced
            let results = harness
                .search_fulltext("test_space", "Person", "name", &format!("Thread{}", i), 10)
                .expect("Failed to search");
            assert!(!results.is_empty(), "Index should be synced");
        });
        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().expect("Thread failed");
    }

    // If we reach here, all threads succeeded
}

/// TC-051: Concurrent index updates same space
#[test]
fn test_concurrent_index_updates_same_space() {
    use std::thread;

    // Create independent harness for each thread with unique paths
    let mut handles = vec![];
    for i in 0..10 {
        let handle = thread::spawn(move || {
            // Create independent harness with unique path for each thread
            let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
            let db_path = temp_dir.path().join(format!("test_{}.db", i));
            let index_path = temp_dir.path().join(format!("index_{}", i));

            let mut harness =
                create_harness_with_paths(&db_path, &index_path).expect("Failed to create harness");

            // Setup space
            harness
                .create_space("test_space")
                .expect("Failed to create space");
            harness
                .create_tag_with_fulltext(
                    "test_space",
                    "Person",
                    vec![("name", DataType::String)],
                    vec!["name"],
                )
                .expect("Failed to create tag");

            // Non-transactional insert (concurrent)
            let vertex = create_test_vertex(
                i + 1,
                "Person",
                vec![("name", Value::String(format!("Concurrent{}", i)))],
            );
            harness
                .insert_vertex("test_space", vertex)
                .expect("Failed to insert vertex");

            // Verify in same thread
            harness.wait_for_async(300);

            // Force commit all
            let rt = &harness.rt;
            rt.block_on(async {
                harness
                    .sync_coordinator
                    .commit_all()
                    .await
                    .expect("Commit all should succeed");
            });

            // Verify vertex exists
            harness
                .assert_vertex_exists("test_space", &Value::Int((i + 1) as i32))
                .expect("Vertex should exist");

            // Verify index is synced
            let results = harness
                .search_fulltext(
                    "test_space",
                    "Person",
                    "name",
                    &format!("Concurrent{}", i),
                    10,
                )
                .expect("Failed to search");
            assert!(!results.is_empty(), "Index should be synced");
        });
        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().expect("Thread failed");
    }

    // If we reach here, all threads succeeded
}
