//! DDL Edge Alter Tests
//!
//! Test coverage:
//! - ALTER EDGE ADD - Add properties to edge
//! - ALTER EDGE DROP - Drop properties from edge
//! - ALTER EDGE CHANGE - Rename properties

use super::common;

use common::test_scenario::TestScenario;
use linkrs_query::parser::Parser;

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
        // 4 property rows + 2 leading endpoint-constraint rows (src_tag/dst_tag).
        .assert_result_count(6);
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
        // 1 remaining property row + 2 leading endpoint-constraint rows.
        .assert_result_count(3);
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

// ==================== ALTER EDGE CHANGE Execution Tests ====================

#[test]
fn test_alter_edge_execution_change_rename() {
    use linkrs_core::Value;
    use std::collections::HashMap;

    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: STRING, old_note: STRING) FROM Person TO Person")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('alice'), 2:('bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since, old_note) VALUES 1 -> 2:('2024-01-01', 'hi')")
        .assert_success()
        .exec_ddl("ALTER EDGE KNOWS CHANGE (old_note note: STRING)")
        .assert_success()
        .query("DESC EDGE KNOWS")
        .assert_success()
        .assert_result_contains(vec![Value::string("note")])
        .assert_edge_props(
            1,
            2,
            "KNOWS",
            HashMap::from([
                ("since", Value::string("2024-01-01")),
                ("note", Value::string("hi")),
            ]),
        )
        .assert_edge_prop_absent(1, 2, "KNOWS", "old_note");
}

#[test]
fn test_alter_edge_execution_change_missing_property_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: STRING) FROM Person TO Person")
        .assert_success()
        .exec_ddl("ALTER EDGE KNOWS CHANGE (missing renamed: STRING)")
        .assert_error();
}
