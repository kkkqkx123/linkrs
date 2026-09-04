//! DQL SUBGRAPH Tests
//!
//! Test coverage:
//! - GET SUBGRAPH parser
//! - GET SUBGRAPH with steps
//! - GET SUBGRAPH with WHERE
//! - GET SUBGRAPH with YIELD
//! - GET SUBGRAPH with edge type filter
//! - GET SUBGRAPH execution

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::parser::Parser;

// ==================== GET SUBGRAPH Parser Tests ====================

#[test]
fn test_get_subgraph_parser() {
    let query = "GET SUBGRAPH FROM 1";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GET SUBGRAPH parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_get_subgraph_with_steps_parser() {
    let query = "GET SUBGRAPH 2 STEPS FROM 1";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GET SUBGRAPH with steps parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_get_subgraph_with_where_parser() {
    let query = "GET SUBGRAPH FROM 1 WHERE $$.Person.age > 25";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GET SUBGRAPH with WHERE parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_get_subgraph_with_yield_parser() {
    let query = "GET SUBGRAPH FROM 1 YIELD vertices, edges";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GET SUBGRAPH with YIELD parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_get_subgraph_with_over_parser() {
    let query = "GET SUBGRAPH FROM 1 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GET SUBGRAPH with OVER parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_get_subgraph_full_syntax_parser() {
    let query =
        "GET SUBGRAPH 3 STEPS FROM 1 OVER KNOWS WHERE $$.Person.age > 25 YIELD vertices, edges";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GET SUBGRAPH full syntax parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== GET SUBGRAPH Execution Tests ====================

#[test]
fn test_get_subgraph_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01'), 2 -> 4:('2022-01-01')")
        .assert_success()
        .query("GET SUBGRAPH FROM 1")
        .assert_success();
}

#[test]
fn test_get_subgraph_multi_steps_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 3:('2021-01-01'), 3 -> 4:('2022-01-01')")
        .assert_success()
        .query("GET SUBGRAPH 3 STEPS FROM 1")
        .assert_success();
}

#[test]
fn test_get_subgraph_single_vertex_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("GET SUBGRAPH FROM 1")
        .assert_success();
}

#[test]
fn test_get_subgraph_with_yield_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("GET SUBGRAPH FROM 1 YIELD vertices, edges")
        .assert_success();
}

#[test]
fn test_get_subgraph_no_edges_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .query("GET SUBGRAPH FROM 1")
        .assert_success();
}

// ==================== Pipe-Subgraph Parser Tests ====================

#[test]
fn test_pipe_into_subgraph_parser() {
    let query = "GO FROM 1 OVER KNOWS | GET SUBGRAPH 2 STEPS FROM $-.id";
    let mut parser = Parser::new(query);
    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Pipe into subgraph parsing should succeed: {:?}",
        result.err()
    );
    let stmt = result.expect("should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PIPE");
}

#[test]
fn test_pipe_into_subgraph_with_over_parser() {
    let query = "GO FROM 1 OVER KNOWS | GET SUBGRAPH 3 STEPS FROM $-.id OVER KNOWS";
    let mut parser = Parser::new(query);
    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Pipe into subgraph with OVER parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_pipe_into_subgraph_with_where_parser() {
    let query = "GO FROM 1 OVER KNOWS | GET SUBGRAPH 2 STEPS FROM $-.id WHERE $$.Person.age > 25";
    let mut parser = Parser::new(query);
    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Pipe into subgraph with WHERE parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_subgraph_pipe_root_is_some() {
    use graphdb_query::parser::Parser;

    let query = "GO FROM 1 OVER KNOWS | GET SUBGRAPH 2 STEPS FROM $-.id";
    let mut parser = Parser::new(query);
    let stmt = parser.parse().expect("parse should succeed");

    // Verify the AST is a PIPE containing a GO and a SUBGRAPH.
    use graphdb_query::parser::ast::Stmt;
    match stmt.ast.stmt() {
        Stmt::Pipe(pipe) => {
            assert!(
                matches!(pipe.left.as_ref(), Stmt::Go(_)),
                "left side should be GO"
            );
            assert!(
                matches!(pipe.right.as_ref(), Stmt::Subgraph(_)),
                "right side should be SUBGRAPH"
            );
        }
        other => panic!("expected PIPE, got {:?}", other),
    }
}
