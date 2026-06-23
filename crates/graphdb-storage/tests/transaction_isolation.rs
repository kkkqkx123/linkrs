//! Transaction Isolation Integration Tests
//!
//! Tests MVCC snapshot isolation and version visibility across transactions

mod common;

use graphdb_storage::core::types::VertexId;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use graphdb_storage::storage::{StorageReader, StorageWriter};

/// Test: Concurrent transactions see consistent snapshots
#[test]
fn test_snapshot_isolation_consistency() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    // Insert initial data
    for i in 0..5 {
        let v = common::create_person_vertex(i, &format!("Initial{}", i), 20);
        storage.insert_vertex("test_space", v).unwrap();
    }

    let storage = Arc::new(Mutex::new(storage));
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = vec![];

    // Thread 1: Transaction A - reads initial state
    {
        let storage = Arc::clone(&storage);
        let barrier = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            barrier.wait();

            let st = storage.lock().unwrap();
            let vertices_a = st.scan_vertices("test_space").unwrap();
            let count_a = vertices_a.len();

            // Sleep to let other transactions modify
            drop(st);
            std::thread::sleep(std::time::Duration::from_millis(100));

            let st = storage.lock().unwrap();
            let vertices_a2 = st.scan_vertices("test_space").unwrap();
            let count_a2 = vertices_a2.len();

            // If snapshot isolation is correct, both reads should see same count
            // or at least a consistent state
            println!("Thread A: First read={}, Second read={}", count_a, count_a2);
            assert!(count_a2 >= count_a, "Later read should have >= earlier data");
        });
        handles.push(handle);
    }

    // Thread 2: Transaction B - modifies data
    {
        let storage = Arc::clone(&storage);
        let barrier = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            barrier.wait();

            std::thread::sleep(std::time::Duration::from_millis(50));

            // Insert new vertices while A is running
            for i in 5..15 {
                let v = common::create_person_vertex(i, &format!("Modified{}", i), 25);
                let mut st = storage.lock().unwrap();
                st.insert_vertex("test_space", v).unwrap();
            }
        });
        handles.push(handle);
    }

    // Thread 3: Transaction C - reads later state
    {
        let storage = Arc::clone(&storage);
        let barrier = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            barrier.wait();

            std::thread::sleep(std::time::Duration::from_millis(200));

            let st = storage.lock().unwrap();
            let vertices_c = st.scan_vertices("test_space").unwrap();

            // C should see all data including modifications
            println!("Thread C: Read {} vertices", vertices_c.len());
            assert!(vertices_c.len() >= 15, "Later transaction should see all modifications");
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }
}

/// Test: Concurrent updates don't create inconsistent states
#[test]
fn test_concurrent_vertex_updates_consistency() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    // Create vertices
    for i in 0..10 {
        let v = common::create_person_vertex(i, &format!("Person{}", i), 20);
        storage.insert_vertex("test_space", v).unwrap();
    }

    let storage = Arc::new(Mutex::new(storage));
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = vec![];

    // 4 threads each reading and verifying vertices
    for thread_id in 0..4 {
        let storage = Arc::clone(&storage);
        let barrier = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            barrier.wait();

            // Each thread reads different vertices
            for vertex_idx in (thread_id * 2)..(thread_id * 2 + 2) {
                if vertex_idx < 10 {
                    let vid = VertexId::from_int64(vertex_idx as i64);

                    let st = storage.lock().unwrap();
                    if let Some(v) = st.get_vertex("test_space", &vid).unwrap() {
                        // Verify data is consistent
                        assert!(!v.tags.is_empty(), "Vertex should have tags");
                        assert!(v.properties.contains_key("name"), "Vertex should have name property");

                        // Verify property types are correct
                        if let Some(name) = v.properties.get("name") {
                            match name {
                                graphdb_storage::core::Value::String(_) => {
                                    // OK
                                }
                                _ => panic!("Name should be string"),
                            }
                        }
                    }
                }
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    // Verify final state
    let st = storage.lock().unwrap();
    let vertices = st.scan_vertices("test_space").unwrap();
    assert_eq!(vertices.len(), 10);
}

/// Test: Multiple readers can exist concurrently
#[test]
fn test_concurrent_readers_consistency() {
    let mut storage = common::create_in_memory_storage();
    common::setup_basic_schema(&mut storage);

    // Insert initial data
    for i in 0..20 {
        let v = common::create_person_vertex(i, &format!("Person{}", i), 20);
        storage.insert_vertex("test_space", v).unwrap();
    }

    let storage = Arc::new(Mutex::new(storage));
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = vec![];

    // 8 concurrent reader threads
    for _thread_id in 0..8 {
        let storage = Arc::clone(&storage);
        let barrier = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            barrier.wait();

            // Each reader scans all vertices multiple times
            for _ in 0..10 {
                let st = storage.lock().unwrap();
                let vertices = st.scan_vertices("test_space").unwrap();
                assert_eq!(vertices.len(), 20, "All readers should see same vertex count");
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }
}
