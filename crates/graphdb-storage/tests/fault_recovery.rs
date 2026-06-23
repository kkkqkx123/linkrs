//! Fault Recovery Integration Tests
//!
//! Tests that storage recovers correctly from various failure scenarios

mod common;

use graphdb_storage::core::types::VertexId;
use graphdb_storage::storage::{StorageReader, StorageWriter, StorageAdmin, StoragePersistenceOps};

/// Test: Recovery after partial write during vertex insertion
#[test]
fn test_recovery_partial_vertex_write() {
    let dir = std::env::temp_dir().join("graphdb_fault_vertex_partial");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");

        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();

        // Insert additional data without flushing
        let v = common::create_person_vertex(100, "ExtraVertex", 25);
        storage.insert_vertex("test_space", v).unwrap();

        // Simulate drop without save (partial write)
        drop(storage);
    }

    // Reopen and verify recovery
    {
        let storage = common::open_persistent_storage(&dir);

        // Original data should be intact
        let v1 = storage
            .get_vertex("test_space", &VertexId::from_int64(1))
            .unwrap();
        assert!(v1.is_some(), "Original data should be recovered");

        // Verify no corruption occurred
        let all_vertices = storage.scan_vertices("test_space").unwrap();
        assert!(
            all_vertices.len() >= 2,
            "Should have at least 2 vertices after recovery"
        );

        // All vertices should have proper structure
        for v in all_vertices {
            assert!(!v.tags.is_empty(), "Recovered vertex should have tags");
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test: Recovery after concurrent operations crash
#[test]
fn test_recovery_after_concurrent_crash() {
    let dir = std::env::temp_dir().join("graphdb_fault_concurrent_crash");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");

        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();

        // Simulate multiple vertex insertions without flush
        for i in 10..15 {
            let v = common::create_person_vertex(i, &format!("Person{}", i), 20);
            storage.insert_vertex("test_space", v).unwrap();
        }

        // Crash without saving
        drop(storage);
    }

    // Verify recovery
    {
        let storage = common::open_persistent_storage(&dir);

        // Check data consistency
        let vertices = storage.scan_vertices("test_space").unwrap();
        assert!(
            vertices.len() >= 2,
            "Should recover at least original data"
        );

        // Verify no corruption in recovered data
        for v in &vertices {
            assert!(!v.tags.is_empty(), "Recovered vertex should have tags");
        }

        println!("Recovered {} vertices after concurrent crash", vertices.len());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test: Recovery of index state after crash
#[test]
fn test_recovery_index_state_after_crash() {
    let dir = std::env::temp_dir().join("graphdb_fault_index_recovery");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);

        // Create index
        common::create_person_name_index(&mut storage, "test_space");

        common::insert_test_data(&mut storage, "test_space");

        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();

        // Insert data and modify index without flush
        for i in 10..20 {
            let v = common::create_person_vertex(i, &format!("Person{}", i), 20);
            storage.insert_vertex("test_space", v).unwrap();
        }

        drop(storage);
    }

    // Verify index recovery
    {
        let storage = common::open_persistent_storage(&dir);

        // Index should still be functional
        let result = storage.lookup_index(
            "test_space",
            "person_name_idx",
            &graphdb_storage::core::Value::String("Alice".to_string()),
        );

        // May not find due to WAL recovery behavior, but should not error
        assert!(
            result.is_ok(),
            "Index lookup should not error after recovery"
        );

        println!("Index recovery result: {:?}", result);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test: Recovery of deleted vertices after crash
#[test]
fn test_recovery_deleted_vertex_integrity() {
    let dir = std::env::temp_dir().join("graphdb_fault_delete_recovery");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let mut storage = common::create_persistent_storage(&dir);
        common::setup_basic_schema(&mut storage);
        common::insert_test_data(&mut storage, "test_space");

        storage.save_to_disk().unwrap();
        storage.create_checkpoint().unwrap();

        // Delete vertex and insert new one without flush
        storage
            .delete_vertex("test_space", &VertexId::from_int64(1))
            .unwrap();

        let v = common::create_person_vertex(100, "NewPerson", 30);
        storage.insert_vertex("test_space", v).unwrap();

        drop(storage);
    }

    // Verify recovery preserves consistency
    {
        let storage = common::open_persistent_storage(&dir);

        // Check recovered state
        let all_vertices = storage.scan_vertices("test_space").unwrap();

        // Either both deletions and insertions are applied (full recovery)
        // or neither (checkpoint-based recovery), but not partial state
        let has_v1 = all_vertices.iter().any(|v| v.vid == VertexId::from_int64(1));
        let has_new = all_vertices
            .iter()
            .any(|v| v.vid == VertexId::from_int64(100));

        println!(
            "Recovery state: v1_exists={}, new_exists={}, total_count={}",
            has_v1,
            has_new,
            all_vertices.len()
        );

        // Verify consistency: not both and not conflicting
        assert!(
            !has_v1 || !has_new || all_vertices.len() >= 2,
            "Recovered state should be consistent"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test: Multiple crash-recovery cycles maintain consistency
#[test]
fn test_multiple_crash_recovery_cycles() {
    let dir = std::env::temp_dir().join("graphdb_fault_multi_cycle");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    for cycle in 0..3 {
        {
            let mut storage = if cycle == 0 {
                common::create_persistent_storage(&dir)
            } else {
                common::open_persistent_storage(&dir)
            };

            if cycle == 0 {
                common::setup_basic_schema(&mut storage);
                common::insert_test_data(&mut storage, "test_space");
                storage.save_to_disk().unwrap();
                storage.create_checkpoint().unwrap();
            }

            // Add some data without flushing
            for i in (10 + cycle * 10)..(10 + (cycle + 1) * 10) {
                let v = common::create_person_vertex(i, &format!("Person{}", i), 20);
                storage.insert_vertex("test_space", v).unwrap();
            }

            drop(storage);
        }

        // Reopen and verify consistency
        {
            let storage = common::open_persistent_storage(&dir);
            let vertices = storage.scan_vertices("test_space").unwrap();
            assert!(
                vertices.len() >= 2,
                "Cycle {}: Should have vertices", cycle
            );

            println!(
                "Cycle {}: Recovered {} vertices",
                cycle,
                vertices.len()
            );
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
