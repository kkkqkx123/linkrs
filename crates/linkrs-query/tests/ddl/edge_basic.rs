//! DDL Edge Basic Tests
//!
//! Test coverage:
//! - CREATE EDGE - Create edge type (including TTL expiry configuration)
//! - DROP EDGE - Delete edge type (single, batch, IF EXISTS)
//! - DESC EDGE - Describe edge schema
//! - TTL - Expiry filtering of expired edges

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::parser::Parser;

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
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success();
}

#[test]
fn test_create_edge_execution_with_if_not_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
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
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
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
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
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
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .exec_ddl("DROP EDGE IF EXISTS KNOWS")
        .assert_success();
}

#[test]
fn test_drop_edge_execution_multiple() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG P1(id: INT, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG P2(id: INT, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE RelA(since: INT) FROM P1 TO P2")
        .assert_success()
        .exec_ddl("CREATE EDGE RelB(since: INT) FROM P1 TO P2")
        .assert_success()
        .exec_ddl("DROP EDGE RelA, RelB")
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
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .query("DESCRIBE EDGE KNOWS")
        .assert_success()
        // 1 property row + 2 leading endpoint-constraint rows (src_tag/dst_tag).
        .assert_result_count(3)
        .assert_result_contains(vec![Value::string("src_tag")])
        .assert_result_contains(vec![Value::string("dst_tag")]);
}

// ==================== Edge Lifecycle Tests ====================

#[test]
fn test_ddl_edge_lifecycle() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
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

// ==================== TTL Parser Tests ====================

#[test]
fn test_create_edge_parser_with_ttl() {
    let query = "CREATE EDGE TempRel(data: STRING, expire_at: INT64, ttl_duration=2592000, ttl_col=expire_at)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE EDGE with TTL parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

// ==================== TTL Execution Tests ====================

#[test]
fn test_create_edge_execution_with_ttl() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG P(pid: INT64, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG Q(pid: INT64, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE TempRel(expire_at: INT64, ttl_duration=100, ttl_col=expire_at) FROM P TO Q")
        .assert_success();
}

#[test]
fn test_create_edge_execution_with_ttl_type_mismatch_rejected() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG P(pid: INT64, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG Q(pid: INT64, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE Bad(label: STRING, ttl_duration=100, ttl_col=label) FROM P TO Q")
        .assert_error();
}

#[test]
fn test_ttl_expired_edge_is_filtered() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG P(pid: INT64, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG Q(pid: INT64, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE TempRel(expire_at: INT64, ttl_duration=100, ttl_col=expire_at) FROM P TO Q")
        .assert_success()
        .exec_dml("INSERT VERTEX P(name) VALUES 1:('a'), 2:('b')")
        .assert_success()
        .exec_dml("INSERT VERTEX Q(name) VALUES 3:('c'), 4:('d')")
        .assert_success()
        .exec_dml("INSERT EDGE TempRel(expire_at) VALUES 1 -> 3:(0)")
        .assert_success()
        .exec_dml("INSERT EDGE TempRel(expire_at) VALUES 2 -> 4:(9999999999)")
        .assert_success()
        .assert_edge_count("TempRel", 1);
}

#[test]
fn test_purge_expired_edge_physical_delete() {
    // Executor-layer edge purge (mirrors the vertex-side coverage): the
    // expired edge is filtered from reads, then PURGE EXPIRED physically
    // deletes it and reports one removed row.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG P(pid: INT64, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG Q(pid: INT64, name: STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE TempRel(expire_at: INT64, ttl_duration=100, ttl_col=expire_at) FROM P TO Q")
        .assert_success()
        .exec_dml("INSERT VERTEX P(name) VALUES 1:('a'), 2:('b')")
        .assert_success()
        .exec_dml("INSERT VERTEX Q(name) VALUES 3:('c'), 4:('d')")
        .assert_success()
        .exec_dml("INSERT EDGE TempRel(expire_at) VALUES 1 -> 3:(0)")
        .assert_success()
        .exec_dml("INSERT EDGE TempRel(expire_at) VALUES 2 -> 4:(9999999999)")
        .assert_success()
        .assert_edge_count("TempRel", 1)
        .exec_ddl("PURGE EXPIRED EDGE TempRel IN test_space")
        .assert_success()
        .assert_result_columns(&["space_name", "schema_name", "schema_type", "removed_rows"])
        .assert_result_contains(vec![
            Value::string("TempRel".to_string()),
            Value::string("EDGE".to_string()),
            Value::BigInt(1),
        ]);
}
