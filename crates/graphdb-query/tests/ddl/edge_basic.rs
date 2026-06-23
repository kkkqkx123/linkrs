//! DDL Edge Basic Tests
//!
//! Test coverage:
//! - CREATE EDGE - Create edge type
//! - DROP EDGE - Delete edge type
//! - DESC EDGE - Describe edge schema

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== CREATE EDGE Parser Tests ====================

#[test]
fn test_create_edge_parser_basic() {
    let query = "CREATE EDGE KNOWS(since: DATE)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE EDGE basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_edge_parser_with_if_not_exists() {
    let query = "CREATE EDGE IF NOT EXISTS KNOWS(since: DATE)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE EDGE with IF NOT EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_edge_parser_single_property() {
    let query = "CREATE EDGE KNOWS(since: DATE)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE EDGE single property parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_edge_parser_multiple_properties() {
    let query = "CREATE EDGE KNOWS(since: DATE, degree: DOUBLE, note: STRING)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE EDGE multiple properties parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_edge_parser_various_types() {
    let query = "CREATE EDGE Test(since: DATE, weight: DOUBLE, active: BOOL, count: INT)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE EDGE various types parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

// ==================== CREATE EDGE Execution Tests ====================

#[test]
fn test_create_edge_execution_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success();
}

#[test]
fn test_create_edge_execution_with_if_not_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS(since: DATE) FROM Person TO Person")
        .assert_success();
}

#[test]
fn test_create_edge_execution_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

// ==================== DROP EDGE Parser Tests ====================

#[test]
fn test_drop_edge_parser_basic() {
    let query = "DROP EDGE KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP EDGE basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP");
}

#[test]
fn test_drop_edge_parser_with_if_exists() {
    let query = "DROP EDGE IF EXISTS KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP EDGE with IF EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP");
}

#[test]
fn test_drop_edge_parser_multiple() {
    let query = "DROP EDGE KNOWS, LIKES, FOLLOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP EDGE multiple edge types parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP");
}

#[test]
fn test_drop_edge_parser_multiple_with_if_exists() {
    let query = "DROP EDGE IF EXISTS KNOWS, LIKES";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP EDGE multiple edge types with IF EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP");
}

// ==================== DROP EDGE Execution Tests ====================

#[test]
fn test_drop_edge_execution_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .exec_ddl("DROP EDGE KNOWS")
        .assert_success();
}

#[test]
fn test_drop_edge_execution_with_if_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP EDGE IF EXISTS NonExistentEdge")
        .assert_success()
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .exec_ddl("DROP EDGE IF EXISTS KNOWS")
        .assert_success();
}

// ==================== DESC EDGE Tests ====================

#[test]
fn test_desc_parser_edge() {
    let query = "DESCRIBE EDGE KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DESCRIBE EDGE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DESCRIBE EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DESC");
}

#[test]
fn test_desc_parser_short_edge() {
    let query = "DESC EDGE KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DESC EDGE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DESC EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DESC");
}

#[test]
fn test_desc_execution_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .query("DESCRIBE EDGE KNOWS")
        .assert_success()
        .assert_result_count(1);
}

// ==================== Edge Lifecycle Tests ====================

#[test]
fn test_ddl_edge_lifecycle() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE TestEdge(since: DATE, weight: DOUBLE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .query("DESCRIBE EDGE TestEdge")
        .assert_success()
        .exec_ddl("ALTER EDGE TestEdge ADD (note: STRING)")
        .assert_success()
        .exec_dml(
            "INSERT EDGE TestEdge(since, weight, note) VALUES 1 -> 2:('2024-01-01', 1.0, 'test')",
        )
        .assert_success()
        .assert_edge_exists(1, 2, "TestEdge")
        .exec_ddl("ALTER EDGE TestEdge DROP (note)")
        .assert_success()
        .exec_ddl("DROP EDGE TestEdge")
        .assert_success();
}
