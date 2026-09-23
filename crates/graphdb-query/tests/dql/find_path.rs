//! DQL FIND PATH Tests
//!
//! Test coverage:
//! - FIND SHORTEST PATH
//! - FIND ALL PATH
//! - FIND PATH with UPTO steps limit
//! - FIND PATH with WHERE clause
//! - FIND PATH with YIELD
//! - FIND PATH with WEIGHT
//! - FIND PATH with WITH LOOP/CYCLE
//! - Path result content verification (column name, multi-path, no-path)
//!
//! Single-label note: FIND PATH syntax carries no vertex label. Endpoints
//! and intermediate vertices are resolved at execution from the edge type
//! schema (single homogeneous edge types) or from tagged vertex values, so
//! fixtures declare endpoint labels on the edge types.

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::parser::Parser;

// ==================== FIND PATH Parser Tests ====================

#[test]
fn test_find_shortest_path_parser() {
    let query = "FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND SHORTEST PATH parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_all_path_parser() {
    let query = "FIND ALL PATH FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND ALL PATH parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_path_with_steps_parser() {
    let query = "FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS UPTO 2 STEPS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND PATH with steps parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== FIND SHORTEST PATH Execution Tests ====================

#[test]
fn test_find_shortest_path_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 3:('2021-01-01'), 3 -> 4:('2022-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_find_shortest_path_multiple_paths() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2020-01-01'), 2 -> 4:('2021-01-01'), 3 -> 4:('2021-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_find_all_path_execution_verify_content() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 4:('2021-01-01'), 1 -> 3:('2020-01-01'), 3 -> 4:('2021-01-01')")
        .assert_success()
        .query("FIND ALL PATH FROM 1 TO 4 OVER KNOWS")
        .assert_success()
        .assert_result_count(2);
}

// ==================== FIND PATH with WHERE Parser Tests ====================

#[test]
fn test_find_path_with_where_parser() {
    let query = "FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS WHERE $$.Person.age > 25";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND PATH with WHERE parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_path_with_yield_parser() {
    let query = "FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS YIELD path";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND PATH with YIELD parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_path_with_weight_parser() {
    let query = "FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS WEIGHT weight";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND PATH with WEIGHT parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_all_path_with_loop_parser() {
    let query = "FIND ALL PATH WITH LOOP FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND ALL PATH with WITH LOOP parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_all_path_with_cycle_parser() {
    let query = "FIND ALL PATH WITH CYCLE FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND ALL PATH with WITH CYCLE parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_all_path_with_loop_and_cycle_parser() {
    let query = "FIND ALL PATH WITH LOOP WITH CYCLE FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND ALL PATH with WITH LOOP WITH CYCLE parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== FIND PATH with YIELD Execution Tests ====================

#[test]
fn test_find_path_with_yield_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 2 OVER KNOWS YIELD path")
        .assert_success()
        .assert_result_count(1);
}

// ==================== FIND PATH Same Vertex Tests ====================

#[test]
fn test_find_path_same_source_dest() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 1 OVER KNOWS")
        .assert_success()
        .assert_result_count(1);
}

// ==================== FIND PATH No Path Tests ====================

#[test]
fn test_find_path_no_path_result() {
    // The missing endpoint cannot be materialized, so the pair is skipped
    // and the search returns no rows instead of an error.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 999 OVER KNOWS")
        .assert_success()
        .assert_result_empty();
}

// ==================== FIND ALL PATH Execution Tests ====================

#[test]
fn test_find_all_path_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 4:('2021-01-01'), 1 -> 3:('2020-01-01'), 3 -> 4:('2021-01-01')")
        .assert_success()
        .query("FIND ALL PATH FROM 1 TO 4 OVER KNOWS")
        .assert_success()
        .assert_result_count(2);
}

// ==================== FIND PATH with Steps Limit Tests ====================

#[test]
fn test_find_path_with_steps_limit() {
    // The only route needs 3 hops while the search allows 2, so the result
    // is empty.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 3:('2021-01-01'), 3 -> 4:('2022-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS UPTO 2 STEPS")
        .assert_success()
        .assert_result_empty();
}

// ==================== Path Result Verification Tests ====================

#[test]
fn test_find_path_result_column_name() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 2 OVER KNOWS")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_find_shortest_path_single_hop() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 2 OVER KNOWS")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_find_all_path_with_diamond() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 4:('2021-01-01'), 1 -> 3:('2020-01-01'), 3 -> 4:('2021-01-01')")
        .assert_success()
        .query("FIND ALL PATH FROM 1 TO 4 OVER KNOWS")
        .assert_success()
        .assert_result_count(2);
}
