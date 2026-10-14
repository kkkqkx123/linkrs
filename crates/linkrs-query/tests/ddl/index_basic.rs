//! DDL Index Basic Tests
//!
//! Test coverage:
//! - CREATE TAG INDEX - Create tag index
//! - CREATE EDGE INDEX - Create edge index
//! - DROP TAG INDEX - Delete tag index, with IF EXISTS
//! - DROP EDGE INDEX - Delete edge index, with IF EXISTS
//! - DROP INDEX - Bare index drop (defaults to tag index), with IF EXISTS

use super::common;

use common::test_scenario::TestScenario;
use linkrs_query::parser::Parser;

// ==================== DROP TAG INDEX Execution Tests ====================

#[test]
fn test_drop_tag_index_execution_if_exists_missing() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP TAG INDEX IF EXISTS no_such_index")
        .assert_success()
        .exec_ddl("DROP TAG INDEX no_such_index")
        .assert_error();
}

#[test]
fn test_drop_tag_index_execution_roundtrip() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Indexed(name: STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG INDEX idx_indexed ON Indexed(name)")
        .assert_success()
        .exec_ddl("DROP TAG INDEX IF EXISTS idx_indexed")
        .assert_success()
        .exec_ddl("DROP TAG INDEX idx_indexed")
        .assert_error();
}

// ==================== DROP EDGE INDEX Execution Tests ====================

#[test]
fn test_drop_edge_index_execution_if_exists_missing() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP EDGE INDEX IF EXISTS no_such_index")
        .assert_success()
        .exec_ddl("DROP EDGE INDEX no_such_index")
        .assert_error();
}

// ==================== DROP INDEX Execution Tests ====================

#[test]
fn test_drop_index_execution_if_exists_missing() {
    let mut parser = Parser::new("DROP INDEX IF EXISTS no_such_index");
    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP INDEX IF EXISTS should parse: {:?}",
        result.err()
    );
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP INDEX IF EXISTS no_such_index")
        .assert_success()
        .exec_ddl("DROP INDEX no_such_index")
        .assert_error();
}
