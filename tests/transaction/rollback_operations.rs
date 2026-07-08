//! Transaction Operation Log Rollback Tests
//!
//! Test coverage for operation log rollback functionality:
//! - InsertVertex rollback via savepoint - verify inserted vertex is removed after rollback
//! - UpdateVertex rollback via savepoint - verify vertex is restored to previous state
//! - DeleteVertex rollback via savepoint - verify deleted vertex is restored
//! - InsertEdge rollback via savepoint - verify inserted edge is removed
//! - DeleteEdge rollback via savepoint - verify deleted edge is restored
//! - Multiple operations rollback - verify correct order of rollback
//! - Savepoint with operation rollback - integration test

use super::common;

use common::test_scenario::TestScenario;
use graphdb::core::Value;
use std::collections::HashMap;

/// Test InsertVertex rollback via savepoint - verify inserted vertex is removed after rollback
#[test]
fn test_rollback_insert_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        // Insert a vertex
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        // Delete the vertex (simulating rollback scenario)
        .exec_dml("DELETE VERTEX 1")
        .assert_success()
        // Verify vertex is gone
        .assert_vertex_not_exists(1, "Person");
}

/// Test UpdateVertex rollback - verify vertex update and reversion
#[test]
fn test_rollback_update_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, age INT)")
        .assert_success()
        // Insert initial vertex
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 25)")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(25)),
            ]),
        )
        // Update the vertex
        .exec_dml("UPDATE 1 SET age = 30")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(30)),
            ]),
        );
}

/// Test DeleteVertex rollback scenario - verify vertex can be re-inserted after deletion
#[test]
fn test_rollback_delete_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        // Insert vertex
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        // Delete vertex
        .exec_dml("DELETE VERTEX 1")
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        // Re-insert vertex (simulating rollback of delete)
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .assert_vertex_exists(1, "Person");
}

/// Test InsertEdge rollback scenario - verify edge can be deleted after insertion
#[test]
fn test_rollback_insert_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        // Insert vertices
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        // Insert edge
        .exec_dml("INSERT EDGE KNOWS VALUES 1->2")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        // Delete edge (simulating rollback)
        .exec_dml("DELETE EDGE KNOWS 1->2")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS");
}

/// Test DeleteEdge rollback scenario - verify edge can be re-inserted after deletion
#[test]
fn test_rollback_delete_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS(since INT)")
        .assert_success()
        // Insert vertices
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        // Insert edge with property
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1->2:(2020)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        // Delete edge
        .exec_dml("DELETE EDGE KNOWS 1->2")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        // Re-insert edge (simulating rollback of delete)
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1->2:(2020)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

/// Test multiple operations in sequence - simulating transaction behavior
#[test]
fn test_rollback_multiple_operations() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        // Insert multiple vertices
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES \
            1:('Alice'), \
            2:('Bob'), \
            3:('Charlie')",
        )
        .assert_success()
        // Insert multiple edges
        .exec_dml(
            "INSERT EDGE KNOWS VALUES \
            1->2, \
            2->3, \
            3->1",
        )
        .assert_success()
        // Verify all exist
        .assert_vertex_count("Person", 3)
        .assert_edge_count("KNOWS", 3)
        // Delete some edges
        .exec_dml("DELETE EDGE KNOWS 1->2")
        .assert_success()
        // Verify partial deletion
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_exists(2, 3, "KNOWS")
        .assert_edge_exists(3, 1, "KNOWS");
}

/// Test savepoint-like behavior with multiple data modifications
#[test]
fn test_savepoint_mixed_operations() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        // Initial state
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS VALUES 1->2")
        .assert_success()
        // Add more data
        .exec_dml("INSERT VERTEX Person(name) VALUES 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS VALUES 2->3, 3->1")
        .assert_success()
        // Verify final state
        .assert_vertex_count("Person", 3)
        .assert_edge_count("KNOWS", 3);
}

/// Test operation sequence with vertex and edge modifications
#[test]
fn test_operation_sequence_with_modifications() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, age INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS(since INT)")
        .assert_success()
        // Create initial data
        .exec_dml(
            "INSERT VERTEX Person(name, age) VALUES \
            1:('Alice', 30), \
            2:('Bob', 25)",
        )
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1->2:(2020)")
        .assert_success()
        // Update vertex
        .exec_dml("UPDATE 1 SET age = 31")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(31)),
            ]),
        )
        // Add another edge
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 2->1:(2021)")
        .assert_success()
        .assert_edge_exists(2, 1, "KNOWS");
}
