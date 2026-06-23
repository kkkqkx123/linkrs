//! DML Insert Edge Tests
//!
//! Test coverage:
//! - INSERT EDGE - Insert edge data
//! - INSERT EDGE IF NOT EXISTS

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== INSERT EDGE Parser Tests ====================

#[test]
fn test_insert_parser_edge() {
    let query = "INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INSERT EDGE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "INSERT");
}

#[test]
fn test_insert_parser_multiple_edges() {
    let query = "INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 1 -> 3:('2024-02-01')";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INSERT multiple edges parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "INSERT");
}

#[test]
fn test_insert_parser_edge_with_rank() {
    let query = "INSERT EDGE KNOWS(since) VALUES 1 -> 2@0:('2024-01-01')";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INSERT EDGE with rank parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "INSERT");
}

// ==================== INSERT EDGE Execution Tests ====================

#[test]
fn test_insert_execution_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

#[test]
fn test_insert_execution_multiple_edges() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 1 -> 3:('2024-02-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 3, "KNOWS")
        .assert_edge_count("KNOWS", 2);
}

// ==================== INSERT EDGE IF NOT EXISTS Tests ====================

#[test]
fn test_insert_edge_if_not_exists_parser() {
    let query = "INSERT EDGE IF NOT EXISTS KNOWS(since) VALUES 1 -> 2:('2024-01-01')";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INSERT EDGE IF NOT EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "INSERT");
}

#[test]
fn test_insert_edge_if_not_exists_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE IF NOT EXISTS KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml("INSERT EDGE IF NOT EXISTS KNOWS(since) VALUES 1 -> 2:('2024-02-01')")
        .assert_success();
}

// ==================== Edge with Properties Tests ====================

#[test]
fn test_insert_edge_with_multiple_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, weight DOUBLE, note STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since, weight, note) VALUES 1 -> 2:('2024-01-01', 0.8, 'close friend')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

// ==================== Edge Rank Tests ====================

#[test]
fn test_insert_edge_with_rank() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since, strength) VALUES 1 -> 2 @0:('2020-01-01', 0.8)")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since, strength) VALUES 1 -> 3 @1:('2021-01-01', 0.9)")
        .assert_success()
        .assert_edge_count("KNOWS", 2);
}

// ==================== Error Handling Tests ====================

#[test]
fn test_insert_edge_nonexistent_vertices() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_error();
}

#[test]
fn test_insert_edge_nonexistent_edge_type() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_error();
}

// ==================== Edge Self-Loop Tests ====================

#[test]
fn test_insert_edge_self_loop() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 1:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 1, "KNOWS");
}

// ==================== Edge NULL Values Tests ====================

#[test]
fn test_insert_edge_with_null_values() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE FOLLOWS(since DATE NULL, weight DOUBLE NULL)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE FOLLOWS(since, weight) VALUES 1 -> 2:(NULL, 0.5)")
        .assert_success()
        .assert_edge_exists(1, 2, "FOLLOWS");
}

// ==================== Edge Negative Rank Tests ====================

#[test]
fn test_insert_edge_positive_rank() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2 @0:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

// ==================== Edge DATE/DATETIME Properties Tests ====================

#[test]
fn test_insert_edge_with_temporal_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE EVENT(start DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE EVENT(start) VALUES 1 -> 2:('2024-06-15')")
        .assert_success()
        .assert_edge_exists(1, 2, "EVENT");
}

// ==================== Edge GEOGRAPHY Type Tests ====================

#[test]
fn test_insert_edge_with_geography_type() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE LOCATED(coordinates GEOGRAPHY)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE LOCATED(coordinates) VALUES 1 -> 2:(NULL)")
        .assert_success()
        .assert_edge_exists(1, 2, "LOCATED");
}

// ==================== Duplicate Edge Error Tests ====================

#[test]
fn test_insert_duplicate_edge_without_if_not_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-02-01')")
        .assert_error();
}

// ==================== Negative Edge Rank Tests ====================

#[test]
fn test_insert_edge_negative_rank() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2 @(-1):('2024-01-01')")
        .assert_success();
}

// ==================== Empty Properties List Tests ====================

#[test]
fn test_insert_edge_empty_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS() VALUES 1 -> 2:()")
        .assert_success();
}

// ==================== Edge Type Mismatch Tests ====================

#[test]
fn test_insert_edge_type_mismatch() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('not_a_date')")
        .assert_error();
}

// ==================== Edge All NULL Properties Tests ====================

#[test]
fn test_insert_edge_all_null_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE NULL, weight DOUBLE NULL)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since, weight) VALUES 1 -> 2:(NULL, NULL)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}
