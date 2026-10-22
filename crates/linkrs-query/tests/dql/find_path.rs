//! DQL FIND PATH Tests
//!
//! Test coverage:
//! - FIND SHORTEST PATH
//! - FIND ALL PATH
//! - FIND PATH with UPTO steps limit
//! - FIND PATH with WHERE clause
//! - FIND PATH with YIELD
//! - FIND PATH with WEIGHT
//! - FIND PATH with WITH LOOP/TRAIL/WALK
//! - Path result content verification (column name, multi-path, no-path)
//!
//! Single-label note: FIND PATH syntax carries no vertex label. Endpoints
//! and intermediate vertices are resolved at execution from the edge type
//! schema (per endpoint side, across all listed edge types) or from tagged
//! vertex values, so fixtures declare endpoint labels on the edge types.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::parser::Parser;

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
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 3:('2021-01-01'), 3 -> 4:('2022-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS")
        .assert_success()
        .assert_result_columns(&["path"])
        .assert_result_count(1);
    assert_eq!(result_paths(&scenario), vec![vec![1, 2, 3, 4]]);
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
fn test_find_all_path_with_cycle_keyword_rejected() {
    // WITH CYCLE was renamed to WITH WALK; the keyword must not survive
    // next to MATCH's `*ACYCLIC`, whose meaning is the opposite.
    let query = "FIND ALL PATH WITH CYCLE FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_err() || parser.has_errors(),
        "FIND ALL PATH with WITH CYCLE must be rejected"
    );
}

#[test]
fn test_find_all_path_with_walk_parser() {
    let query = "FIND ALL PATH WITH WALK FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND ALL PATH with WITH WALK parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_all_path_with_trail_parser() {
    let query = "FIND ALL PATH WITH TRAIL FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND ALL PATH with WITH TRAIL parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_find_all_path_with_loop_and_walk_parser() {
    let query = "FIND ALL PATH WITH LOOP WITH WALK FROM 1 TO 4 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "FIND ALL PATH with WITH LOOP WITH WALK parsing should succeed: {:?}",
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
        .assert_result_count(1)
        .assert_result_columns(&["path"]);
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
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 2 OVER KNOWS")
        .assert_success()
        .assert_result_columns(&["path"])
        .assert_result_count(1);
    assert_eq!(result_paths(&scenario), vec![vec![1, 2]]);
}

#[test]
fn test_find_shortest_path_single_hop() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("FIND SHORTEST PATH FROM 1 TO 2 OVER KNOWS")
        .assert_success()
        .assert_result_columns(&["path"])
        .assert_result_count(1);
    assert_eq!(result_paths(&scenario), vec![vec![1, 2]]);
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

// ==================== Path Content Helpers ====================

/// Vertex-id sequence of a path value, or `None` for other value kinds.
fn path_vertex_ids(value: &Value) -> Option<Vec<i64>> {
    let Value::Path(path) = value else {
        return None;
    };
    let mut ids = vec![path.src.vid.as_int64().expect("path vertex id")];
    ids.extend(
        path.steps
            .iter()
            .map(|step| step.dst.vid.as_int64().expect("path vertex id")),
    );
    Some(ids)
}

/// Vertex-id sequences of every path value in the last result, row order.
fn result_paths(scenario: &TestScenario) -> Vec<Vec<i64>> {
    let result = scenario.last_result().expect("query produced a result");
    let data = match result {
        ExecutionResult::DataSet { data, .. } => data,
        other => panic!("expected a data set, got {:?}", other),
    };
    data.rows
        .iter()
        .map(|row| {
            row.iter()
                .find_map(path_vertex_ids)
                .expect("every FIND PATH row carries a path value")
        })
        .collect()
}

/// Projected name and edge endpoints of a MATCH row, if present.
fn row_name_and_edge(row: &[Value]) -> (Option<String>, Option<(i64, i64)>) {
    let mut name = None;
    let mut edge = None;
    for value in row {
        match value {
            Value::String(s) => name = Some(s.to_string()),
            Value::Edge(e) => {
                edge = Some((
                    e.src().as_int64().expect("edge src id"),
                    e.dst().as_int64().expect("edge dst id"),
                ));
            }
            _ => {}
        }
    }
    (name, edge)
}

// ==================== Loop/Cycle Semantics Fixtures ====================

/// 1 -> 2, 2 -> 1 (cycle back to the start) and 2 -> 3: reaching 3 through
/// vertex 1 again is only legal when vertices may repeat.
fn repeat_vertex_graph() -> TestScenario {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 1:('2021-01-01'), 2 -> 3:('2022-01-01')")
        .assert_success()
}

/// Vertex 1 carries a self-loop (1 -> 1) and also reaches 2.
fn self_loop_graph() -> TestScenario {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 1:('2020-01-01'), 1 -> 2:('2021-01-01')")
        .assert_success()
}

/// Two parallel edge types between 1 and 2 plus a single type from 2 to 3:
/// reaching 3 through 1 -> 2 -> 1 -> 2 needs four *distinct* edges, which
/// only the trail tier admits (vertex-unique rejects, walk admits plus more).
fn two_edge_type_graph() -> TestScenario {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_ddl("CREATE EDGE LIKES(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 3:('2022-01-01')")
        .exec_dml("INSERT EDGE LIKES(since) VALUES 2 -> 1:('2021-01-01'), 1 -> 2:('2021-06-01')")
        .assert_success()
}

// ==================== Loop/Cycle Semantics Tests ====================

#[test]
fn test_find_all_path_default_prunes_repeated_vertices() {
    let scenario = repeat_vertex_graph()
        .query("FIND ALL PATH FROM 1 TO 3 OVER KNOWS UPTO 5 STEP")
        .assert_success();
    assert_eq!(
        result_paths(&scenario),
        vec![vec![1, 2, 3]],
        "the default policy rejects the path that revisits vertices"
    );
}

#[test]
fn test_find_all_path_with_walk_allows_repeated_vertices() {
    let scenario = repeat_vertex_graph()
        .query("FIND ALL PATH WITH WALK FROM 1 TO 3 OVER KNOWS UPTO 5 STEP")
        .assert_success();
    let mut paths = result_paths(&scenario);
    paths.sort();
    assert_eq!(
        paths,
        vec![vec![1, 2, 1, 2, 3], vec![1, 2, 3]],
        "WITH WALK must admit the path that revisits vertices 1 and 2"
    );
}

#[test]
fn test_find_all_path_with_loop_keeps_vertex_uniqueness() {
    // WITH LOOP only exempts self-loop edges; the 1 -> 2 -> 1 cycle stays
    // rejected, so the result matches the default mode.
    let scenario = repeat_vertex_graph()
        .query("FIND ALL PATH WITH LOOP FROM 1 TO 3 OVER KNOWS UPTO 5 STEP")
        .assert_success();
    assert_eq!(result_paths(&scenario), vec![vec![1, 2, 3]]);
}

#[test]
fn test_find_all_path_default_prunes_repeated_vertices_two_types() {
    // The round trip through vertex 1 needs four distinct parallel edges;
    // vertex uniqueness rejects it, leaving both 1 -> 2 -> 3 variants.
    let scenario = two_edge_type_graph()
        .query("FIND ALL PATH FROM 1 TO 3 OVER KNOWS, LIKES UPTO 5 STEP")
        .assert_success();
    let mut paths = result_paths(&scenario);
    paths.sort();
    assert_eq!(
        paths,
        vec![vec![1, 2, 3], vec![1, 2, 3]],
        "the default policy rejects the path that revisits vertices 1 and 2"
    );
}

#[test]
fn test_find_all_path_with_trail_allows_repeated_vertices() {
    // Trail forbids repeating an edge, not a vertex: the round trip
    // 1 -> 2 -> 1 -> 2 uses two distinct parallel edges per direction.
    let scenario = two_edge_type_graph()
        .query("FIND ALL PATH WITH TRAIL FROM 1 TO 3 OVER KNOWS, LIKES UPTO 5 STEP")
        .assert_success();
    let mut paths = result_paths(&scenario);
    paths.sort();
    assert_eq!(
        paths,
        vec![
            vec![1, 2, 1, 2, 3],
            vec![1, 2, 1, 2, 3],
            vec![1, 2, 3],
            vec![1, 2, 3],
        ],
        "WITH TRAIL must admit the vertex-repeating, edge-unique paths"
    );
}

#[test]
fn test_find_all_path_with_walk_repeats_edges() {
    // Walk drops both constraints: the round trip also exists with the
    // outgoing edge reused, so it appears once more per parallel edge.
    let scenario = two_edge_type_graph()
        .query("FIND ALL PATH WITH WALK FROM 1 TO 3 OVER KNOWS, LIKES UPTO 5 STEP")
        .assert_success();
    let mut paths = result_paths(&scenario);
    paths.sort();
    assert_eq!(
        paths,
        vec![
            vec![1, 2, 1, 2, 3],
            vec![1, 2, 1, 2, 3],
            vec![1, 2, 1, 2, 3],
            vec![1, 2, 1, 2, 3],
            vec![1, 2, 3],
            vec![1, 2, 3],
        ],
        "WITH WALK must admit the edge-reusing round trips as well"
    );
}

#[test]
fn test_find_all_path_self_loop_policies() {
    // WITH LOOP emits the self-loop as a terminal extension: exactly one
    // path [1, 1], never an unbounded stack of self-loops.
    let scenario = self_loop_graph()
        .query("FIND ALL PATH WITH LOOP FROM 1 TO 1 OVER KNOWS UPTO 3 STEP")
        .assert_success();
    assert_eq!(result_paths(&scenario), vec![vec![1, 1]]);

    // WITH WALK releases vertex uniqueness, so the self-loop is legal too.
    let scenario = scenario
        .query("FIND ALL PATH WITH WALK FROM 1 TO 1 OVER KNOWS UPTO 3 STEP")
        .assert_success();
    assert_eq!(result_paths(&scenario), vec![vec![1, 1]]);

    // The default policy prunes the self-loop entirely.
    let scenario = scenario
        .query("FIND ALL PATH FROM 1 TO 1 OVER KNOWS UPTO 3 STEP")
        .assert_success()
        .assert_result_empty();

    // A self-loop adds no reachable vertex, so it never appears on a path
    // whose target lies beyond it.
    let scenario = scenario
        .query("FIND ALL PATH WITH LOOP FROM 1 TO 2 OVER KNOWS UPTO 3 STEP")
        .assert_success();
    assert_eq!(result_paths(&scenario), vec![vec![1, 2]]);
}

// ==================== Multi Edge Type Endpoint Resolution ====================

#[test]
fn test_find_shortest_path_over_multiple_edge_types() {
    // No single edge type spans 1 to 3; the endpoint labels must be
    // resolved across all listed types instead of giving up.
    let scenario = two_edge_type_graph()
        .query("FIND SHORTEST PATH FROM 1 TO 3 OVER KNOWS, LIKES")
        .assert_success();
    assert_eq!(result_paths(&scenario), vec![vec![1, 2, 3]]);
}

#[test]
fn test_find_path_bare_id_without_edge_types_errors() {
    // Without OVER the edge schema cannot determine the endpoint label of
    // a literal id; that is an explicit error, not an empty result.
    let scenario = two_edge_type_graph()
        .query("FIND SHORTEST PATH FROM 1 TO 3")
        .assert_error();
    let error = scenario.error().unwrap_or_default();
    assert!(
        error.contains("bare id"),
        "expected the bare-id endpoint error, got: {error}"
    );
}

// ==================== Weighted Shortest Path Tests ====================

/// The direct hop 1 -> 4 costs 10 while the three-hop route 1 -> 2 -> 3 -> 4
/// costs 0.5 per edge, so cost order and hop order disagree.
fn weighted_graph() -> TestScenario {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(w DOUBLE) FROM Person TO Person")
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')",
        )
        .exec_dml(
            "INSERT EDGE KNOWS(w) VALUES 1 -> 4:(10.0), 1 -> 2:(0.5), 2 -> 3:(0.5), 3 -> 4:(0.5)",
        )
        .assert_success()
}

/// The direct hop carries a weight of 0.5; the detour hops carry NULL, which
/// is non-numeric and therefore costs the default 1.0.
fn missing_weight_graph() -> TestScenario {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(w DOUBLE NULL) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(w) VALUES 1 -> 4:(0.5), 1 -> 2:(NULL), 2 -> 4:(NULL)")
        .assert_success()
}

/// Unclamped, the detour 1 -> 2 -> 4 would cost -10 + 5 = -5 and beat the
/// direct hop's 3; clamped to 0.0 it costs 5 and loses.
fn negative_weight_graph() -> TestScenario {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(w DOUBLE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(w) VALUES 1 -> 4:(3.0), 1 -> 2:(-10.0), 2 -> 4:(5.0)")
        .assert_success()
}

/// Edge ranks steer the search: the direct hop ranks 10, the detour ranks 1.
fn ranking_graph() -> TestScenario {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 4 @10:('2020-01-01'), 1 -> 2 @1:('2021-01-01'), 2 -> 4 @1:('2022-01-01')")
        .assert_success()
}

#[test]
fn test_find_shortest_path_weight_prefers_cheaper_route() {
    let scenario = weighted_graph()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS WEIGHT w")
        .assert_success();
    assert_eq!(
        result_paths(&scenario),
        vec![vec![1, 2, 3, 4]],
        "the cheaper three-hop route must beat the direct hop"
    );
}

#[test]
fn test_find_shortest_path_unweighted_ignores_edge_weights() {
    let scenario = weighted_graph()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS")
        .assert_success();
    assert_eq!(result_paths(&scenario), vec![vec![1, 4]]);
}

#[test]
fn test_find_all_path_with_weight_is_rejected() {
    let scenario = weighted_graph().query("FIND ALL PATH FROM 1 TO 4 OVER KNOWS WEIGHT w");
    let err = scenario
        .error()
        .expect("WEIGHT on FIND ALL PATH must fail at bind time");
    assert!(
        err.contains("WEIGHT is only supported for FIND SHORTEST PATH"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_find_shortest_path_weight_missing_defaults_to_one() {
    // The direct hop costs 0.5; the detour hops are non-numeric (NULL), so
    // each costs 1.0 and the direct hop wins. A 0.0 default would flip the
    // winner to the two-hop detour.
    let scenario = missing_weight_graph()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS WEIGHT w")
        .assert_success();
    assert_eq!(result_paths(&scenario), vec![vec![1, 4]]);
}

#[test]
fn test_find_shortest_path_weight_negative_clamped_to_zero() {
    let scenario = negative_weight_graph()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS WEIGHT w")
        .assert_success();
    assert_eq!(
        result_paths(&scenario),
        vec![vec![1, 4]],
        "negative weights clamp to 0.0, making the detour cost 5 and lose"
    );
}

#[test]
fn test_find_shortest_path_weight_ranking_uses_rank_field() {
    let scenario = ranking_graph()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS WEIGHT ranking")
        .assert_success();
    assert_eq!(
        result_paths(&scenario),
        vec![vec![1, 2, 4]],
        "ranking weights make the detour (rank 1 + 1) beat the direct hop (rank 10)"
    );
}

#[test]
fn test_find_path_weight_matches_match_weighted() {
    // Both weighted engines must pick the same route on the same graph.
    let scenario = weighted_graph()
        .query("FIND SHORTEST PATH FROM 1 TO 4 OVER KNOWS WEIGHT w")
        .assert_success();
    assert_eq!(result_paths(&scenario), vec![vec![1, 2, 3, 4]]);

    let scenario = scenario
        .query("MATCH (a:Person)-[e:KNOWS*WEIGHTED(w)]->(b:Person) RETURN b.name, e")
        .assert_success();
    let result = scenario.last_result().expect("match produced a result");
    let data = match result {
        ExecutionResult::DataSet { data, .. } => data,
        other => panic!("expected a data set, got {:?}", other),
    };
    let mut cheap_hop_seen = false;
    for row in &data.rows {
        let (name, edge) = row_name_and_edge(row);
        if name.as_deref() != Some("David") {
            continue;
        }
        assert_eq!(
            edge,
            Some((3, 4)),
            "MATCH *WEIGHTED must reach David through the cheap 3 -> 4 hop"
        );
        cheap_hop_seen = true;
    }
    assert!(
        cheap_hop_seen,
        "MATCH *WEIGHTED must reach David at least once"
    );
}
