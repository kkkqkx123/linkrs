//! Integration tests for P4 row-level conflict detection
//!
//! Tests the interaction between write set tracking, conflict detection, and MVCC

use std::time::Duration;

use crate::transaction::manager::TransactionManager;
use crate::transaction::types::{
    DurabilityLevel, TransactionId, TransactionManagerConfig, TransactionOptions,
};
use crate::core::types::VertexId;
use crate::transaction::WriteSetAnalyzer;

fn create_test_manager() -> TransactionManager {
    let config = TransactionManagerConfig {
        auto_cleanup: false,
        ..Default::default()
    };
    TransactionManager::new(config)
}

/// Test 3.1.1: Multiple transactions with non-conflicting writes
#[test]
fn test_multiple_non_conflicting_writes() {
    let manager = create_test_manager();

    // Create 3 write transactions with different vertices
    let txn1 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn1");

    let txn2 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn2");

    let txn3 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn3");

    let ctx1 = manager.get_context(txn1).expect("Failed to get ctx1");
    let ctx2 = manager.get_context(txn2).expect("Failed to get ctx2");
    let ctx3 = manager.get_context(txn3).expect("Failed to get ctx3");

    // Record writes on different vertices
    ctx1.record_vertex_write(VertexId::from_int64(1));
    ctx2.record_vertex_write(VertexId::from_int64(2));
    ctx3.record_vertex_write(VertexId::from_int64(3));

    // All should pass conflict check
    assert!(manager.check_write_set_conflict(txn1).is_ok());
    assert!(manager.check_write_set_conflict(txn2).is_ok());
    assert!(manager.check_write_set_conflict(txn3).is_ok());

    manager.commit_transaction(txn1).unwrap();
    manager.commit_transaction(txn2).unwrap();
    manager.commit_transaction(txn3).unwrap();
}

/// Test 3.1.2: Two transactions with conflicting writes on same vertex
#[test]
fn test_conflicting_writes_same_vertex() {
    let manager = create_test_manager();

    let txn1 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn1");

    let txn2 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn2");

    let ctx1 = manager.get_context(txn1).expect("Failed to get ctx1");
    let ctx2 = manager.get_context(txn2).expect("Failed to get ctx2");

    let vid = VertexId::from_int64(1);
    ctx1.record_vertex_write(vid);
    ctx2.record_vertex_write(vid);

    // txn1 should pass first
    assert!(manager.check_write_set_conflict(txn1).is_ok());

    // txn2 should fail due to conflict with txn1
    assert!(manager.check_write_set_conflict(txn2).is_err());

    manager.commit_transaction(txn1).unwrap();
    manager.commit_transaction(txn2).unwrap();
}

/// Test 3.1.3: Conflict intensity measurement
#[test]
fn test_conflict_intensity_varying_overlaps() {
    use crate::transaction::types::WriteSet;

    // No overlap: intensity = 0.0
    let ws1 = {
        let mut ws = WriteSet::new();
        ws.record_vertex(VertexId::from_int64(1));
        ws
    };

    let ws2 = {
        let mut ws = WriteSet::new();
        ws.record_vertex(VertexId::from_int64(2));
        ws
    };

    assert_eq!(WriteSetAnalyzer::conflict_intensity(&ws1, &ws2), 0.0);

    // 50% overlap: intensity = 0.5
    let ws3 = {
        let mut ws = WriteSet::new();
        ws.record_vertex(VertexId::from_int64(1)); // overlap with ws1
        ws.record_vertex(VertexId::from_int64(3)); // no overlap
        ws
    };

    let intensity = WriteSetAnalyzer::conflict_intensity(&ws1, &ws3);
    assert!(intensity > 0.3 && intensity < 0.7); // approximately 0.5

    // 100% overlap: intensity = 1.0
    let ws4 = {
        let mut ws = WriteSet::new();
        ws.record_vertex(VertexId::from_int64(1));
        ws
    };

    assert_eq!(WriteSetAnalyzer::conflict_intensity(&ws1, &ws4), 1.0);
}

/// Test 3.1.4: ConflictReport detailed analysis
#[test]
fn test_conflict_report_classification() {
    use crate::transaction::types::WriteSet;
    use crate::core::types::EdgeIdentifier;

    let vid1 = VertexId::from_int64(1);
    let vid2 = VertexId::from_int64(2);
    let vid3 = VertexId::from_int64(3);

    // Vertex conflict
    let mut ws_v1 = WriteSet::new();
    ws_v1.record_vertex(vid1);

    let mut ws_v2 = WriteSet::new();
    ws_v2.record_vertex(vid1); // same vertex

    let report_v = WriteSetAnalyzer::analyze_conflict(&ws_v1, &ws_v2);
    assert!(report_v.has_conflict);
    assert!(report_v.vertex_conflict);
    assert!(!report_v.edge_conflict);

    // Edge conflict (same edge)
    let mut ws_e1 = WriteSet::new();
    let edge1 = EdgeIdentifier::new(1, vid1, 1, vid2, 1, 0);
    ws_e1.record_edge(edge1);

    let mut ws_e2 = WriteSet::new();
    ws_e2.record_edge(edge1); // same edge

    let report_e = WriteSetAnalyzer::analyze_conflict(&ws_e1, &ws_e2);
    assert!(report_e.has_conflict);
    assert!(report_e.edge_conflict);
    assert!(!report_e.vertex_conflict);

    // Shared vertex endpoint conflict
    let mut ws_e3 = WriteSet::new();
    let edge2 = EdgeIdentifier::new(1, vid1, 1, vid2, 1, 0);
    let edge3 = EdgeIdentifier::new(1, vid1, 1, vid3, 1, 0); // shares src vertex
    ws_e3.record_edge(edge2);

    let mut ws_e4 = WriteSet::new();
    ws_e4.record_edge(edge3);

    let report_shared = WriteSetAnalyzer::analyze_conflict(&ws_e3, &ws_e4);
    assert!(report_shared.has_conflict);
    assert!(report_shared.shared_vertex_conflict);
}

/// Test 3.1.5: Read-only transaction doesn't conflict
#[test]
fn test_readonly_transaction_no_conflict() {
    let manager = create_test_manager();

    let txn_write = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin write txn");

    let txn_read = manager
        .begin_read_transaction(TransactionOptions::default())
        .expect("Failed to begin read txn");

    let ctx_write = manager.get_context(txn_write).expect("Failed to get write ctx");
    ctx_write.record_vertex_write(VertexId::from_int64(1));

    // Read transaction should not cause conflict
    assert!(manager.check_write_set_conflict(txn_write).is_ok());
    assert!(manager.check_write_set_conflict(txn_read).is_ok());

    manager.commit_transaction(txn_write).unwrap();
    manager.commit_transaction(txn_read).unwrap();
}

/// Test 3.1.6: Empty write set (no writes recorded)
#[test]
fn test_empty_write_set_no_conflict() {
    let manager = create_test_manager();

    let txn1 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn1");

    let txn2 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn2");

    // Don't record any writes - both should have empty write sets

    // Both should pass conflict check (no actual conflicts)
    assert!(manager.check_write_set_conflict(txn1).is_ok());
    assert!(manager.check_write_set_conflict(txn2).is_ok());

    manager.commit_transaction(txn1).unwrap();
    manager.commit_transaction(txn2).unwrap();
}

/// Test 3.1.7: Write set size tracking
#[test]
fn test_write_set_size_tracking() {
    let manager = create_test_manager();

    let txn = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn");

    let ctx = manager.get_context(txn).expect("Failed to get ctx");

    // Initially empty
    assert_eq!(ctx.write_set_size(), 0);
    assert!(ctx.is_write_set_empty());

    // Record vertex writes
    for i in 1..=5 {
        ctx.record_vertex_write(VertexId::from_int64(i));
    }

    assert_eq!(ctx.write_set_size(), 5);
    assert!(!ctx.is_write_set_empty());

    manager.commit_transaction(txn).unwrap();
}

/// Test 3.1.8: Sequential conflict detection across multiple transactions
#[test]
fn test_sequential_conflict_cascade() {
    let manager = create_test_manager();

    // Create chain: txn1 -> txn2 -> txn3
    let txn1 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn1");

    let txn2 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn2");

    let txn3 = manager
        .begin_insert_transaction(TransactionOptions::default())
        .expect("Failed to begin txn3");

    let ctx1 = manager.get_context(txn1).expect("Failed to get ctx1");
    let ctx2 = manager.get_context(txn2).expect("Failed to get ctx2");
    let ctx3 = manager.get_context(txn3).expect("Failed to get ctx3");

    // All write to same vertex
    let vid = VertexId::from_int64(1);
    ctx1.record_vertex_write(vid);
    ctx2.record_vertex_write(vid);
    ctx3.record_vertex_write(vid);

    // txn1 should pass
    assert!(manager.check_write_set_conflict(txn1).is_ok());

    // txn2 should fail (conflicts with txn1)
    assert!(manager.check_write_set_conflict(txn2).is_err());

    // txn3 should also fail (conflicts with txn1)
    assert!(manager.check_write_set_conflict(txn3).is_err());

    manager.commit_transaction(txn1).unwrap();
    manager.commit_transaction(txn2).unwrap();
    manager.commit_transaction(txn3).unwrap();
}
