//! Transaction Edge Operation Tests
//!
//! Test coverage:
//! - Edge creation in transaction
//! - Edge deletion in transaction
//! - Self-referencing edge
//! - Bidirectional edges
//! - Multiple edge types
//! - Edge direction validation

use super::common;

use common::test_scenario::TestScenario;

/// Test edge creation in transaction
#[test]
fn test_transaction_edge_creation() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS(since INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1->2:(2020)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

/// Test edge deletion in transaction
#[test]
fn test_transaction_edge_delete() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS VALUES 1->2")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml("DELETE EDGE KNOWS 1->2")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS");
}

/// Test transaction with self-referencing edge
#[test]
fn test_transaction_self_referencing_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS VALUES 1->1")
        .assert_success()
        .assert_edge_exists(1, 1, "KNOWS");
}

/// Test transaction with bidirectional edges
#[test]
fn test_transaction_bidirectional_edges() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS FRIENDS_WITH")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE FRIENDS_WITH VALUES 1->2")
        .assert_success()
        .exec_dml("INSERT EDGE FRIENDS_WITH VALUES 2->1")
        .assert_success()
        .assert_edge_exists(1, 2, "FRIENDS_WITH")
        .assert_edge_exists(2, 1, "FRIENDS_WITH");
}

/// Test transaction with multiple edge types
#[test]
fn test_transaction_multiple_edge_types() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS(since INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS WORKS_WITH(project STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1->2:(2020)")
        .assert_success()
        .exec_dml("INSERT EDGE WORKS_WITH(project) VALUES 1->2:('ProjectX')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 2, "WORKS_WITH");
}

/// Test transaction with edge direction
#[test]
fn test_transaction_edge_direction() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS FOLLOWS")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE FOLLOWS VALUES 1->2")
        .assert_success()
        .assert_edge_exists(1, 2, "FOLLOWS")
        .assert_edge_not_exists(2, 1, "FOLLOWS");
}
