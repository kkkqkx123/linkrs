//! DQL Set Operations Tests
//!
//! Test coverage:
//! - UNION - Combine results from two queries
//! - UNION ALL - Combine results including duplicates
//! - INTERSECT - Common results from two queries
//! - MINUS - Results in first query but not in second

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== Set Operations Parser Tests ====================

#[test]
fn test_union_parser() {
    let query =
        "GO FROM 1 OVER KNOWS YIELD $$.Person.name AS name UNION GO FROM 2 OVER KNOWS YIELD $$.Person.name AS name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNION parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_union_all_parser() {
    let query =
        "GO FROM 1 OVER KNOWS YIELD $$.Person.name AS name UNION ALL GO FROM 2 OVER KNOWS YIELD $$.Person.name AS name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNION ALL parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_intersect_parser() {
    let query =
        "GO FROM 1 OVER KNOWS YIELD $$.Person.name AS name INTERSECT GO FROM 2 OVER KNOWS YIELD $$.Person.name AS name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INTERSECT parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_minus_parser() {
    let query =
        "GO FROM 1 OVER KNOWS YIELD $$.Person.name AS name MINUS GO FROM 2 OVER KNOWS YIELD $$.Person.name AS name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MINUS parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_union_with_match_parser() {
    let query = "MATCH (a:Person) RETURN a.name UNION MATCH (b:Person) RETURN b.name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNION with MATCH parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_multiple_union_parser() {
    let query = "GO FROM 1 OVER KNOWS YIELD $$.Person.name AS name UNION GO FROM 2 OVER KNOWS YIELD $$.Person.name AS name UNION ALL GO FROM 3 OVER KNOWS YIELD $$.Person.name AS name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Multiple UNION parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_union_with_lookup_parser() {
    let query = "LOOKUP ON Person YIELD Person.name UNION LOOKUP ON Person YIELD Person.name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNION with LOOKUP parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_union_with_fetch_parser() {
    let query =
        "FETCH PROP ON Person 1, 2 YIELD Person.name UNION FETCH PROP ON Person 3, 4 YIELD Person.name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNION with FETCH parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== UNION with MATCH Execution Tests ====================

#[test]
fn test_union_with_match_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 OR id(v) == 2 RETURN v.name AS name UNION MATCH (v:Person) WHERE id(v) == 2 OR id(v) == 3 RETURN v.name AS name")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_union_all_with_match_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 OR id(v) == 2 RETURN v.name AS name UNION ALL MATCH (v:Person) WHERE id(v) == 2 OR id(v) == 3 RETURN v.name AS name")
        .assert_success()
        .assert_result_count(4);
}

#[test]
fn test_intersect_with_match_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 OR id(v) == 2 RETURN v.name AS name INTERSECT MATCH (v:Person) WHERE id(v) == 2 OR id(v) == 3 RETURN v.name AS name")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_minus_with_match_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 OR id(v) == 2 RETURN v.name AS name MINUS MATCH (v:Person) WHERE id(v) == 2 OR id(v) == 3 RETURN v.name AS name")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_minus_all_different() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 OR id(v) == 2 RETURN v.name AS name MINUS MATCH (v:Person) WHERE id(v) == 3 RETURN v.name AS name")
        .assert_success()
        .assert_result_count(2);
}

// ==================== Set Operations with Empty Results ====================

#[test]
fn test_intersect_empty_result() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 RETURN v.name AS name INTERSECT MATCH (v:Person) WHERE id(v) == 999 RETURN v.name AS name")
        .assert_success()
        .assert_result_empty();
}
