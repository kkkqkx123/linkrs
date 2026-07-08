//! DML Upsert Tests
//!
//! Test coverage:
//! - UPSERT VERTEX - Insert or update vertex
//! - UPSERT EDGE - Insert or update edge
//! - MERGE - Merge operation

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::core::Value;
use graphdb_query::query::parser::Parser;
use std::collections::HashMap;

// ==================== UPSERT VERTEX Parser Tests ====================

#[test]
fn test_upsert_parser_vertex() {
    let query = "UPSERT VERTEX ON Person SET name = 'Alice', age = 30 WHERE id(vid) == 1";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPSERT VERTEX parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPSERT");
}

#[test]
fn test_upsert_parser_vertex_with_when() {
    let query = "UPSERT VERTEX ON Person SET age = age + 1 WHERE id(vid) == 1 WHEN age < 100";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPSERT with WHEN parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPSERT");
}

#[test]
fn test_upsert_parser_vertex_with_yield() {
    let query = "UPSERT VERTEX ON Person SET name = 'Bob' WHERE id(vid) == 1 YIELD name, age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPSERT with YIELD parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPSERT");
}

// ==================== UPSERT VERTEX Execution Tests ====================

#[test]
fn test_upsert_execution_vertex_insert() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("UPSERT VERTEX ON Person SET name = 'Alice', age = 30 WHERE id(vid) == 1")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(30)),
            ]),
        );
}

#[test]
fn test_upsert_execution_vertex_update() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .exec_dml("UPSERT VERTEX ON Person SET name = 'Bob', age = 35 WHERE id(vid) == 1")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Bob".into())),
                ("age", Value::Int(35)),
            ]),
        );
}

#[test]
fn test_upsert_vertex_update_partial() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT, city STRING)")
        .exec_dml("INSERT VERTEX Person(name, age, city) VALUES 1:('Alice', 30, 'NYC')")
        .assert_success()
        .exec_dml("UPSERT VERTEX ON Person SET age = 31 WHERE id(vid) == 1")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(31)),
                ("city", Value::String("NYC".into())),
            ]),
        );
}

// ==================== UPSERT EDGE Tests ====================

#[test]
fn test_upsert_parser_edge() {
    let query = "UPSERT EDGE ON KNOWS SET since = '2024-01-01' WHERE id(src) == 1 AND id(dst) == 2";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPSERT EDGE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPSERT");
}

#[test]
fn test_upsert_execution_edge_insert() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml(
            "UPSERT EDGE ON KNOWS SET since = '2024-01-01' WHERE id(src) == 1 AND id(dst) == 2",
        )
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

#[test]
fn test_upsert_execution_edge_update() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .exec_dml(
            "UPSERT EDGE ON KNOWS SET since = '2024-02-01' WHERE id(src) == 1 AND id(dst) == 2",
        )
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

// ==================== UPSERT EDGE Insert Tests ====================

#[test]
fn test_upsert_edge_replaces_existing() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since, strength) VALUES 1 -> 2:('2020-01-01', 0.5)")
        .assert_success()
        .exec_dml(
            "UPSERT EDGE ON KNOWS SET since = '2024-01-01', strength = 0.9 WHERE id(src) == 1 AND id(dst) == 2",
        )
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

// ==================== MERGE Execution Tests ====================

#[test]
fn test_merge_parser_vertex() {
    let query = "MERGE (v:Person {name: 'Alice'}) SET v.age = 30";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MERGE VERTEX parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MERGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "MERGE");
}

#[test]
fn test_merge_parser_edge() {
    let query = "MERGE (a)-[r:KNOWS {since: '2024-01-01'}]->(b) SET r.weight = 1.0";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MERGE EDGE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MERGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "MERGE");
}

#[test]
fn test_merge_execution_vertex_create() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("MERGE (v:Person {name: 'Alice'}) SET v.age = 30")
        .assert_success();
}

#[test]
fn test_merge_execution_vertex_match() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 25)")
        .assert_success()
        .exec_dml("MERGE (v:Person {name: 'Alice'}) SET v.age = 30")
        .assert_success();
}

// ==================== UPSERT with WHEN Condition Execution Tests ====================

#[test]
fn test_upsert_when_condition_update() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .exec_dml("UPSERT VERTEX ON Person SET age = age + 1 WHERE id(vid) == 1 WHEN age < 40")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(31)),
            ]),
        );
}

#[test]
fn test_upsert_when_condition_skipped() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 50)")
        .assert_success()
        .exec_dml("UPSERT VERTEX ON Person SET age = age + 1 WHERE id(vid) == 1 WHEN age < 40")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(51)),
            ]),
        );
}

// ==================== UPSERT with YIELD Execution Tests ====================

#[test]
fn test_upsert_yield_parser() {
    let query = "UPSERT VERTEX ON Person SET name = 'Bob' WHERE id(vid) == 1 YIELD name, age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPSERT with YIELD parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPSERT");
}

#[test]
fn test_upsert_yield_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("UPSERT VERTEX ON Person SET name = 'Alice', age = 30 WHERE id(vid) == 1 YIELD name, age")
        .assert_success();
}

// ==================== UPSERT Arithmetic Expression Tests ====================

#[test]
fn test_upsert_arithmetic_insert() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Counter(val INT)")
        .exec_dml("UPSERT VERTEX ON Counter SET val = 10 WHERE id(vid) == 1")
        .assert_success()
        .exec_dml("UPSERT VERTEX ON Counter SET val = val + 5 WHERE id(vid) == 1")
        .assert_success()
        .assert_vertex_props(1, "Counter", HashMap::from([("val", Value::Int(15))]));
}

// ==================== MERGE EDGE Execution Tests ====================

#[test]
fn test_merge_edge_execution_create() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("UPSERT EDGE ON KNOWS SET since = '2024-01-01', strength = 1.0 WHERE id(src) == 1 AND id(dst) == 2")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

#[test]
fn test_merge_edge_execution_existing() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, weight DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since, weight) VALUES 1 -> 2:('2020-01-01', 0.5)")
        .assert_success()
        .exec_dml("MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) MERGE (a)-[r:KNOWS]->(b) SET r.weight = 1.0")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

// ==================== UPSERT Error Handling Tests ====================

#[test]
fn test_upsert_nonexistent_tag() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_dml("UPSERT VERTEX ON NonExistent SET name = 'test' WHERE id(vid) == 1")
        .assert_error();
}

#[test]
fn test_upsert_nonexistent_edge_type() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("UPSERT EDGE ON NonExistent SET since = '2024-01-01' WHERE id(src) == 1 AND id(dst) == 2")
        .assert_error();
}

// ==================== MERGE EDGE ON ... SET ... WHERE ... Tests ====================

#[test]
fn test_merge_edge_on_parser() {
    let query = "MERGE EDGE ON KNOWS SET since = '2024-01-01', strength = 1.0 WHERE id(src) == 1 AND id(dst) == 2";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MERGE EDGE ON ... SET ... WHERE ... parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MERGE EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPSERT");
}

#[test]
fn test_merge_vertex_on_parser() {
    let query = "MERGE VERTEX ON Person SET name = 'Alice', age = 30 WHERE id(vid) == 1";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MERGE VERTEX ON ... SET ... WHERE ... parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MERGE VERTEX statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPSERT");
}

#[test]
fn test_merge_edge_on_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("MERGE EDGE ON KNOWS SET since = '2024-01-01', strength = 1.0 WHERE id(src) == 1 AND id(dst) == 2")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

#[test]
fn test_merge_edge_on_update_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since, strength) VALUES 1 -> 2:('2020-01-01', 0.5)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml("MERGE EDGE ON KNOWS SET since = '2024-06-01', strength = 0.9 WHERE id(src) == 1 AND id(dst) == 2")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

// ==================== UPSERT EDGE with Rank Tests ====================

#[test]
fn test_upsert_edge_with_rank() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("UPSERT EDGE ON KNOWS SET since = '2024-01-01', strength = 0.5 WHERE id(src) == 1 AND id(dst) == 2 @0")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

// ==================== UPSERT WHERE No Match Tests ====================

#[test]
fn test_upsert_where_no_match() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .exec_dml("UPSERT VERTEX ON Person SET name = 'Bob', age = 25 WHERE id(vid) == 999")
        .assert_success()
        .assert_vertex_exists(999, "Person")
        .assert_vertex_props(
            999,
            "Person",
            HashMap::from([
                ("name", Value::String("Bob".into())),
                ("age", Value::Int(25)),
            ]),
        );
}

// ==================== UPSERT EDGE YIELD Tests ====================

#[test]
fn test_upsert_edge_yield_parser() {
    let query = "UPSERT EDGE ON KNOWS SET since = '2024-01-01' WHERE id(src) == 1 AND id(dst) == 2 YIELD since";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPSERT EDGE with YIELD parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPSERT EDGE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPSERT");
}

#[test]
fn test_upsert_edge_yield_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("UPSERT EDGE ON KNOWS SET since = '2024-01-01' WHERE id(src) == 1 AND id(dst) == 2 YIELD since")
        .assert_success();
}
