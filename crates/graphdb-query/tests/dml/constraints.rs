//! DML Constraint Tests
//!
//! Test coverage:
//! - NOT NULL constraint - Insert without required property
//! - NOT NULL constraint - Insert NULL into NOT NULL column
//! - DEFAULT values - Insert with default values
//! - DEFAULT values - Explicit override of default values

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== NOT NULL Constraint Parser Tests ====================

#[test]
fn test_not_null_parser_create_tag() {
    let query = "CREATE TAG Person(name STRING NOT NULL, age INT)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with NOT NULL parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== NOT NULL Constraint Execution Tests ====================

#[test]
fn test_insert_not_null_success() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING NOT NULL, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .assert_vertex_exists(1, "Person");
}

// ==================== NOT NULL Constraint Violation Tests ====================

#[test]
fn test_insert_not_null_violation_explicit_null() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING NOT NULL, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:(NULL, 30)")
        .assert_error();
}

#[test]
fn test_insert_not_null_violation_omitted_field() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING NOT NULL, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(age) VALUES 1:(30)")
        .assert_error();
}

#[test]
fn test_insert_multiple_not_null_columns() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING NOT NULL, email STRING NOT NULL, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, email, age) VALUES 1:('Alice', 'alice@test.com', 30)")
        .assert_success()
        .assert_vertex_exists(1, "Person");
}

#[test]
fn test_insert_multiple_not_null_violation() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING NOT NULL, email STRING NOT NULL, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_error();
}

// ==================== DEFAULT Value Parser Tests ====================

#[test]
fn test_default_parser_create_tag() {
    let query = "CREATE TAG Person(name STRING DEFAULT 'unknown', age INT DEFAULT 18)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with DEFAULT parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== DEFAULT Value Execution Tests ====================

#[test]
fn test_insert_default_uses_default_value() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING DEFAULT 'Anonymous', age INT DEFAULT 18)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .assert_vertex_exists(1, "Person");
}

#[test]
fn test_insert_default_overrides_default() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING DEFAULT 'Anonymous', age INT DEFAULT 18)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Bob', 25)")
        .assert_success()
        .assert_vertex_exists(1, "Person");
}

#[test]
fn test_insert_default_all_defaults() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING DEFAULT 'Anonymous', age INT DEFAULT 30)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Charlie')")
        .assert_success()
        .assert_vertex_exists(1, "Person");
}
