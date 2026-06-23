//! DQL FETCH Query Tests
//!
//! Test coverage:
//! - FETCH PROP ON - Fetch vertex properties
//! - FETCH PROP ON with multiple vertices
//! - FETCH PROP ON edge

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== FETCH Parser Tests ====================

#[test]
fn test_fetch_parser_vertex() {
    let query = "FETCH PROP ON Person 1";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FETCH PROP ON vertex parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("FETCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "FETCH");
}

#[test]
fn test_fetch_parser_multiple_vertices() {
    let query = "FETCH PROP ON Person 1, 2, 3";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FETCH PROP ON multiple vertices parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("FETCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "FETCH");
}

#[test]
fn test_fetch_parser_with_yield() {
    let query = "FETCH PROP ON Person 1 YIELD name, age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FETCH with YIELD parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("FETCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "FETCH");
}

#[test]
fn test_fetch_parser_edge() {
    let query = "FETCH PROP ON KNOWS 1 -> 2";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FETCH PROP ON edge parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("FETCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "FETCH");
}

#[test]
fn test_fetch_parser_all_tags() {
    let query = "FETCH PROP ON * 1";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FETCH PROP ON * parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("FETCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "FETCH");
}

// ==================== FETCH Execution Tests ====================

#[test]
fn test_fetch_execution_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("FETCH PROP ON Person 1")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_fetch_execution_multiple_vertices() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("FETCH PROP ON Person 1, 2, 3")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_fetch_execution_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .query("FETCH PROP ON KNOWS 1 -> 2")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_fetch_execution_with_yield() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("FETCH PROP ON Person 1 YIELD Person.name AS name, Person.age AS age")
        .assert_success()
        .assert_result_count(1);
}

// ==================== FETCH Edge Property Tests ====================

#[test]
fn test_fetch_execution_edge_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since, strength) VALUES 1 -> 2:('2020-01-01', 0.9)")
        .assert_success()
        .query("FETCH PROP ON KNOWS 1 -> 2")
        .assert_success()
        .assert_result_count(1)
        .assert_vertex_or_edge_has_property(
            "since",
            graphdb_query::core::Value::Date(graphdb_query::core::DateValue {
                year: 2020,
                month: 1,
                day: 1,
            }),
        );
}

#[test]
fn test_fetch_execution_vertex_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT, city STRING)")
        .exec_dml("INSERT VERTEX Person(name, age, city) VALUES 1:('Alice', 30, 'NYC')")
        .assert_success()
        .query("FETCH PROP ON Person 1")
        .assert_success()
        .assert_result_count(1)
        .assert_vertex_or_edge_has_property(
            "name",
            graphdb_query::core::Value::String("Alice".into()),
        )
        .assert_vertex_or_edge_has_property("age", graphdb_query::core::Value::Int(30));
}

// ==================== FETCH Error Handling Tests ====================

#[test]
fn test_fetch_nonexistent_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .query("FETCH PROP ON Person 999")
        .assert_success()
        .assert_result_count(0);
}

#[test]
fn test_fetch_nonexistent_tag() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("FETCH PROP ON NonExistentTag 1")
        .assert_error();
}
