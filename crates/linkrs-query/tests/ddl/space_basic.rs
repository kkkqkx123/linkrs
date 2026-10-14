//! DDL Space Basic Tests
//!
//! Test coverage:
//! - CREATE SPACE - Create graph space
//! - DROP SPACE - Delete graph space, with IF EXISTS
//! - DROP GRAPH - Namespace alias for DROP SPACE

use super::common;

use common::test_scenario::TestScenario;

// ==================== DROP SPACE Execution Tests ====================

#[test]
fn test_drop_space_execution_if_exists_missing() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP SPACE IF EXISTS missing_space")
        .assert_success();
}

#[test]
fn test_drop_space_execution_missing_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP SPACE missing_space")
        .assert_error();
}

// ==================== DROP GRAPH Execution Tests ====================

#[test]
fn test_drop_graph_execution_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("graph_space")
        .exec_ddl("DROP GRAPH graph_space")
        .assert_success()
        .exec_ddl("DROP SPACE graph_space")
        .assert_error();
}

#[test]
fn test_drop_graph_execution_missing_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP GRAPH no_such_space")
        .assert_error();
}

#[test]
fn test_drop_graph_execution_if_exists_missing() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP GRAPH IF EXISTS no_such_space")
        .assert_success();
}
