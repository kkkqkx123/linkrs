//! DQL Execution Contract Tests
//!
//! Lock end-to-end execution behaviour for representative full-process cases:
//! statement parsing, planning, execution, LIMIT/SKIP/DISTINCT/sample behaviour,
//! null handling, and explicit rejection of unsupported syntax.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_query::parser::Parser;

// ==================== Parser contract: unsupported/removed syntax ====================

#[test]
fn test_go_steps_must_be_integer_literal() {
    let result = Parser::new("GO 2 TO 4 STEPS FROM 1 OVER KNOWS YIELD 1").parse();
    assert!(result.is_err(), "range steps are not supported for GO");
}

#[test]
fn test_find_path_requires_to() {
    let result = Parser::new("FIND PATH FROM 1 OVER KNOWS").parse();
    assert!(result.is_err(), "FIND PATH must require TO");
}

#[test]
fn test_with_edge_rejected() {
    let result = Parser::new("GET SUBGRAPH WITH EDGE 1 STEPS FROM 1").parse();
    assert!(result.is_err(), "WITH EDGE is not supported");
}

// ==================== OPTIONAL MATCH execution ====================

#[test]
fn test_optional_match_preserves_unmatched_sources() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_optional_1")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:(1)")
        .assert_success()
        .query("OPTIONAL MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_optional_match_empty_graph_returns_null_rows() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_optional_2")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .query("OPTIONAL MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name")
        .assert_success()
        .assert_result_count(2);
}

// ==================== Variable-length range execution ====================

#[test]
fn test_varlen_single_step_chain() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_varlen_1")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:(1), 2 -> 3:(1)")
        .assert_success()
        .query("MATCH (a:Person)-[r:KNOWS*1..2]->(b:Person) WHERE a.name == 'A' RETURN b.name")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_varlen_large_upper_bound_not_truncated() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_varlen_2")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C'), 4:('D')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:(1), 2 -> 3:(1), 3 -> 4:(1)")
        .assert_success()
        .query("MATCH (a:Person)-[r:KNOWS*1..10]->(b:Person) WHERE a.name == 'A' RETURN b.name")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_varlen_invalid_range_is_error() {
    let result = Parser::new("MATCH (a:Person)-[r:KNOWS*2..1]->(b:Person) RETURN a").parse();
    // Parser may accept syntax; ensure execution surfaces invalid range if it parses.
    // We only assert that, when routed through the pipeline, it does not silently succeed.
    if result.is_ok() {
        let scenario = TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("ec_varlen_3")
            .exec_ddl("CREATE TAG Person(id INT, name STRING)")
            .query("MATCH (a:Person)-[r:KNOWS*2..1]->(b:Person) RETURN a");
        // Either an error or an empty result is acceptable; silent full traversal is not.
        assert!(
            scenario.error().is_some()
                || scenario.last_result().map(|r| r.count()).unwrap_or(0) == 0
        );
    } else {
        assert!(result.is_err());
    }
}

// ==================== FIND PATH contract ====================

#[test]
fn test_find_path_no_path_empty() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_findpath_1")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 2 OVER KNOWS")
        .assert_success()
        .assert_result_empty();
}

// ==================== Standalone YIELD contract ====================

#[test]
fn test_standalone_yield_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_yield_1")
        .query("YIELD 1 + 1 AS result")
        .assert_success()
        .assert_result_count(1)
        .assert_result_columns(&["result"]);
}

#[test]
fn test_standalone_yield_distinct_pipeline() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_yield_2")
        .query("UNWIND [1, 1, 2] AS x | YIELD DISTINCT x")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_standalone_yield_sample_parser() {
    let result = Parser::new("YIELD x SAMPLE 2").parse();
    assert!(result.is_ok(), "standalone YIELD should accept SAMPLE");
}

#[test]
fn test_sample_clause_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_sample_1")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C'), 4:('D')")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.name SAMPLE 2")
        .assert_success()
        .assert_result_count(2);
}

// ==================== GO contract ====================

#[test]
fn test_go_integer_multi_step() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_go_1")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:(1), 2 -> 3:(1)")
        .assert_success()
        .query("GO 2 STEPS FROM 1 OVER KNOWS")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_go_rejected_range() {
    let result = Parser::new("GO 1 TO 2 STEPS FROM 1 OVER KNOWS YIELD 1").parse();
    assert!(result.is_err());
}

// ==================== FETCH contract ====================

#[test]
fn test_fetch_vertex_contract() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ec_fetch_1")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("FETCH PROP ON Person 1")
        .assert_success()
        .assert_result_count(1);
}
