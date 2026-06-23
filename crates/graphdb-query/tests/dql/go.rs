//! DQL GO Query Tests
//!
//! Test coverage:
//! - GO FROM - Basic traversal
//! - GO with multiple steps
//! - GO with WHERE clause
//! - GO with YIELD

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== GO Parser Tests ====================

#[test]
fn test_go_parser_basic() {
    let query = "GO FROM 1 OVER knows";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GO basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GO statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GO");
}

#[test]
fn test_go_parser_with_yield() {
    let query = "GO FROM 1 OVER knows YIELD $$.person.name, $$.person.age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GO with YIELD parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GO statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GO");
}

#[test]
fn test_go_parser_with_where() {
    let query = "GO FROM 1 OVER knows WHERE $$.person.age > 25";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GO with WHERE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GO statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GO");
}

#[test]
fn test_go_parser_multi_steps() {
    let query = "GO 2 STEPS FROM 1 OVER knows";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GO multi steps parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GO statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GO");
}

#[test]
fn test_go_parser_reverse() {
    let query = "GO FROM 1 OVER knows REVERSELY";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GO REVERSELY parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GO statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GO");
}

#[test]
fn test_go_parser_bidirect() {
    let query = "GO FROM 1 OVER knows BIDIRECT";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GO BIDIRECT parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GO statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GO");
}

// ==================== GO Execution Tests ====================

#[test]
fn test_go_execution_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 1 -> 3:('2024-01-02')")
        .assert_success()
        .query("GO FROM 1 OVER KNOWS")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_go_execution_with_yield() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .query("GO FROM 1 OVER KNOWS YIELD $$.Person.name AS name, $$.Person.age AS age")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_go_execution_multi_steps() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C'), 4:('D')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 2 -> 3:('2024-01-02'), 3 -> 4:('2024-01-03')")
        .assert_success()
        .query("GO 2 STEPS FROM 1 OVER KNOWS")
        .assert_success();
}

// ==================== GO REVERSELY and BIDIRECT Tests ====================

#[test]
fn test_go_execution_reversely() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE FOLLOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE FOLLOWS(since) VALUES 2 -> 1:('2020-01-01'), 3 -> 1:('2021-01-01')")
        .assert_success()
        .query("GO FROM 1 OVER FOLLOWS REVERSELY YIELD $^.Person.name AS follower")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_go_execution_bidirect() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE FRIEND(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE FRIEND(since) VALUES 1 -> 2:('2020-01-01'), 3 -> 1:('2021-01-01')")
        .assert_success()
        .query("GO FROM 1 OVER FRIEND BIDIRECT YIELD $$.Person.name AS friend")
        .assert_success()
        .assert_result_count(2);
}

// ==================== GO Multi-Step Traversal Tests ====================

#[test]
fn test_go_execution_basic_traversal() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01'), 2 -> 4:('2022-01-01')")
        .assert_success()
        .query("GO FROM 1 OVER KNOWS YIELD $$.Person.name AS friend_name")
        .assert_success()
        .assert_result_count(2)
        .query("GO 2 FROM 1 OVER KNOWS YIELD $$.Person.name AS friend_of_friend")
        .assert_success()
        .assert_result_count(1);
}

// ==================== GO Error Handling Tests ====================

#[test]
fn test_go_nonexistent_source() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .query("GO FROM 999 OVER KNOWS")
        .assert_success()
        .assert_result_count(0);
}

#[test]
fn test_go_nonexistent_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .query("GO FROM 1 OVER NONEXISTENT_EDGE")
        .assert_error();
}
