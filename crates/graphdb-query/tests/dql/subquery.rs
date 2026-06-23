//! DQL Subquery Tests
//!
//! Test coverage:
//! - Nested queries
//! - WITH clause
//! - UNWIND clause
//! - Complex query patterns

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== WITH Parser Tests ====================

#[test]
fn test_with_parser_basic() {
    let query = "MATCH (v:Person) WITH v.name AS name RETURN name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITH basic parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_with_parser_aggregation() {
    let query = "MATCH (v:Person) WITH COUNT(v) AS total RETURN total";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITH aggregation parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_with_parser_multiple() {
    let query = "MATCH (v:Person) WITH v.name AS name, v.age AS age RETURN name, age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITH multiple fields parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== UNWIND Parser Tests ====================

#[test]
fn test_unwind_parser_basic() {
    let query = "UNWIND [1, 2, 3] AS x RETURN x";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNWIND basic parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_unwind_parser_with_match() {
    let query = "UNWIND [1, 2, 3] AS id MATCH (v:Person) WHERE id(v) == id RETURN v";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNWIND with MATCH parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== Complex Query Parser Tests ====================

#[test]
fn test_complex_query_parser() {
    let query = r#"
        MATCH (a:Person)-[r:KNOWS]->(b:Person)
        WHERE a.age > 25
        WITH a.name AS from_name, b.name AS to_name, r.since AS since
        RETURN from_name, to_name, since
        ORDER BY since DESC
        LIMIT 10
    "#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Complex query parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== Subquery Execution Tests ====================

#[test]
fn test_with_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)")
        .assert_success()
        .query("MATCH (v:Person) WITH v.name AS name, v.age AS age RETURN name, age")
        .assert_success();
}

#[test]
fn test_unwind_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("UNWIND [1, 2, 3] AS x RETURN x")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_complex_query_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 1 -> 3:('2024-02-01')")
        .assert_success()
        .query(r#"
            MATCH (a:Person)-[r:KNOWS]->(b:Person)
            RETURN a.name, b.name, r.since
            ORDER BY r.since DESC
            LIMIT 5
        "#)
        .assert_success();
}

// ==================== UNWIND with MATCH Tests ====================

#[test]
fn test_unwind_with_match_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, tags STRING)")
        .exec_dml("INSERT VERTEX Person(name, tags) VALUES 1:('Alice', 'friend,colleague'), 2:('Bob', 'family')")
        .assert_success()
        .query("MATCH (n:Person) UNWIND split(n.tags, ',') AS tag RETURN n.name, tag")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_unwind_empty_list() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("UNWIND [] AS n RETURN n")
        .assert_success()
        .assert_result_empty();
}

// ==================== WITH and ORDER BY Tests ====================

#[test]
fn test_with_order_by_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
        .assert_success()
        .query("MATCH (n:Person) WITH n.name AS name, n.age AS age RETURN name, age ORDER BY age DESC LIMIT 2")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_with_where_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
        .assert_success()
        .query("MATCH (n:Person) WITH n.age AS age WHERE age > 25 RETURN age")
        .assert_success()
        .assert_result_count(2);
}

// ==================== Error Handling Tests ====================

#[test]
fn test_invalid_subquery_syntax() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("MATCH (v:Person) WITH RETURN v")
        .assert_error();
}

#[test]
fn test_invalid_unwind_syntax() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("UNWIND [1, 2, 3] RETURN x")
        .assert_error();
}
