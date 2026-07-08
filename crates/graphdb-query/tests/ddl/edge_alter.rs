//! DDL Edge Alter Tests
//!
//! Test coverage:
//! - ALTER EDGE ADD - Add properties to edge
//! - ALTER EDGE DROP - Drop properties from edge
//! - ALTER EDGE CHANGE - Rename properties

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== ALTER EDGE Parser Tests ====================

#[test]
fn test_alter_edge_parser_add() {
    let query = "ALTER EDGE KNOWS ADD (note: STRING, weight: DOUBLE)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ALTER EDGE ADD parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("ALTER EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ALTER");
}

#[test]
fn test_alter_edge_parser_drop() {
    let query = "ALTER EDGE KNOWS DROP (temp_field, old_field)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ALTER EDGE DROP parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("ALTER EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ALTER");
}

#[test]
fn test_alter_edge_parser_change() {
    let query = "ALTER EDGE KNOWS CHANGE (old_since new_since: DATE)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ALTER EDGE CHANGE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("ALTER EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ALTER");
}

#[test]
fn test_alter_edge_parser_add_single() {
    let query = "ALTER EDGE KNOWS ADD (note: STRING)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ALTER EDGE ADD single property parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("ALTER EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ALTER");
}

#[test]
fn test_alter_edge_parser_drop_single() {
    let query = "ALTER EDGE KNOWS DROP (temp_field)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ALTER EDGE DROP single property parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("ALTER EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ALTER");
}

// ==================== ALTER EDGE Execution Tests ====================

#[test]
fn test_alter_edge_execution_add() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .exec_ddl("ALTER EDGE KNOWS ADD (note: STRING)")
        .assert_success();
}

#[test]
fn test_alter_edge_execution_drop() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE, temp_field: STRING) FROM Person TO Person")
        .assert_success()
        .exec_ddl("ALTER EDGE KNOWS DROP (temp_field)")
        .assert_success();
}

#[test]
fn test_alter_edge_execution_add_multiple() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .exec_ddl("ALTER EDGE KNOWS ADD (note: STRING, weight: DOUBLE, verified: BOOL)")
        .assert_success()
        .query("DESCRIBE EDGE KNOWS")
        .assert_success()
        .assert_result_count(4);
}

#[test]
fn test_alter_edge_execution_drop_multiple() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl(
            "CREATE EDGE KNOWS(since: DATE, temp1: STRING, temp2: STRING) FROM Person TO Person",
        )
        .assert_success()
        .exec_ddl("ALTER EDGE KNOWS DROP (temp1, temp2)")
        .assert_success()
        .query("DESCRIBE EDGE KNOWS")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_alter_edge_nonexistent() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("ALTER EDGE NonExistentEdge ADD (field: STRING)")
        .assert_error();
}

#[test]
fn test_alter_edge_drop_nonexistent_field() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .exec_ddl("ALTER EDGE KNOWS DROP (nonexistent_field)")
        .assert_error();
}
