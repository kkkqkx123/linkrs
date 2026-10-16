//! DQL MATCH Query Tests
//!
//! Test coverage:
//! - MATCH - Pattern matching
//! - MATCH with WHERE clause
//! - MATCH with RETURN
//! - MATCH with multiple patterns
//! - OPTIONAL MATCH
//! - MATCH with complex WHERE conditions
//! - MATCH with DISTINCT

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::parser::Parser;

// ==================== MATCH Parser Tests ====================

#[test]
fn test_match_parser_basic() {
    let query = "MATCH (v:Person) RETURN v";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MATCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "MATCH");
}

#[test]
fn test_match_parser_with_edge() {
    let query = "MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a, b, r";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH with edge parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MATCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "MATCH");
}

#[test]
fn test_match_parser_with_where() {
    let query = "MATCH (v:Person) WHERE v.age > 25 RETURN v.name, v.age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH with WHERE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MATCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "MATCH");
}

#[test]
fn test_match_parser_multi_hop() {
    let query = "MATCH (a)-[r1:KNOWS]->(b)-[r2:KNOWS]->(c) RETURN a, c";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH multi-hop parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MATCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "MATCH");
}

#[test]
fn test_match_parser_bidirectional() {
    let query = "MATCH (a)-[r:KNOWS]-(b) RETURN a, b";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH bidirectional parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MATCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "MATCH");
}

#[test]
fn test_match_parser_with_properties() {
    let query = "MATCH (v:Person {name: 'Alice'}) RETURN v";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH with properties parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("MATCH statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "MATCH");
}

// ==================== MATCH Execution Tests ====================

#[test]
fn test_match_execution_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.name AS name")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_match_execution_with_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .query("MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a.name, b.name")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_match_execution_with_where() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 20), 3:('Charlie', 35)")
        .assert_success()
        .query("MATCH (v:Person) WHERE v.age > 25 RETURN v.name, v.age")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_match_execution_with_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)")
        .assert_success()
        .query("MATCH (v:Person {name: 'Alice'}) RETURN v.name, v.age")
        .assert_success()
        .assert_result_count(1);
}

// ==================== MATCH with ORDER BY and LIMIT Tests ====================

#[test]
fn test_match_execution_order_by_and_limit() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35), 4:('David', 28)")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.name, v.age ORDER BY v.age ASC")
        .assert_success()
        .assert_result_count(4)
        .query("MATCH (v:Person) RETURN v.name, v.age ORDER BY v.age DESC LIMIT 2")
        .assert_success()
        .assert_result_count(2);
}

// ==================== MATCH Edge Traversal Tests ====================

#[test]
fn test_match_execution_edge_traversal() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01')")
        .assert_success()
        .query("MATCH (a:Person)-[:KNOWS]->(b:Person) WHERE a.name == 'Alice' RETURN b.name")
        .assert_success()
        .assert_result_count(2)
        .query("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name")
        .assert_success()
        .assert_result_count(2);
}

// ==================== MATCH Complex Query Tests ====================

#[test]
fn test_match_complex_social_network() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("social_network")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT, city STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name, age, city) VALUES 1:('Alice', 30, 'NYC'), 2:('Bob', 25, 'LA'), 3:('Charlie', 35, 'NYC'), 4:('David', 28, 'LA')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since, strength) VALUES 1 -> 2:('2020-01-01', 0.9), 1 -> 3:('2021-01-01', 0.8), 2 -> 4:('2022-01-01', 0.7), 3 -> 4:('2022-01-01', 0.9)")
        .assert_success()
        .query("MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person) WHERE a.name == 'Alice' AND c.city == 'LA' RETURN DISTINCT c.name, c.age")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![linkrs_core::Value::string("David"), linkrs_core::Value::Int(28)]);
}

// ==================== MATCH Edge Cases Tests ====================

#[test]
fn test_match_empty_result() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("MATCH (n:Person) WHERE n.age > 100 RETURN n")
        .assert_success()
        .assert_result_empty();
}

#[test]
fn test_match_large_limit() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("MATCH (n:Person) RETURN n LIMIT 1000")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_match_zero_limit() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .query("MATCH (n:Person) RETURN n LIMIT 0")
        .assert_success()
        .assert_result_empty();
}

#[test]
fn test_match_self_loop() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE SELF_LOOP(notes STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .exec_dml("INSERT EDGE SELF_LOOP(notes) VALUES 1 -> 1:('self reference')")
        .assert_success()
        .query("MATCH (n:Person)-[:SELF_LOOP]->(n:Person) RETURN n.name")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_match_multiple_edge_types() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_ddl("CREATE EDGE FOLLOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .exec_dml("INSERT EDGE FOLLOWS(since) VALUES 1 -> 3:('2021-01-01')")
        .assert_success()
        .query("MATCH (n:Person)-[:KNOWS|:FOLLOWS]->(m:Person) RETURN n.name, m.name")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_match_deep_traversal() {
    let child = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            TestScenario::new()
                .expect("Failed to create test scenario")
                .setup_space("test_space")
                .exec_ddl("CREATE TAG Person(id INT, name STRING)")
                .exec_ddl("CREATE EDGE KNOWS(since DATE)")
                .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David'), 5:('Eve')")
                .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 3:('2020-02-01'), 3 -> 4:('2020-03-01'), 4 -> 5:('2020-04-01')")
                .assert_success()
                .query("MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person)-[:KNOWS]->(d:Person)-[:KNOWS]->(e:Person) WHERE a.name == 'Alice' RETURN e.name")
                .assert_success()
                .assert_result_count(1);
        })
        .expect("Failed to spawn thread");
    child.join().expect("Thread panicked");
}

// ==================== MATCH Error Handling Tests ====================

#[test]
fn test_match_nonexistent_tag() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("MATCH (v:NonExistentTag) RETURN v")
        .assert_error();
}

#[test]
fn test_match_invalid_pattern() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("MATCH (a)-[r]-> RETURN a")
        .assert_error();
}

// ==================== OPTIONAL MATCH Tests ====================

#[test]
fn test_optional_match_parser() {
    let query = "OPTIONAL MATCH (v:Person) RETURN v";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "OPTIONAL MATCH parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_optional_match_edge_parser() {
    let query = "OPTIONAL MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a, b";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "OPTIONAL MATCH with edge parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_optional_match_where_parser() {
    let query = "OPTIONAL MATCH (v:Person) WHERE v.age > 25 RETURN v";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "OPTIONAL MATCH with WHERE parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== Complex WHERE Tests ====================

#[test]
fn test_match_where_and_or_parser() {
    let query = "MATCH (v:Person) WHERE v.age > 25 AND v.city == 'NYC' RETURN v";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH with AND/OR parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_match_distinct_parser() {
    let query = "MATCH (v:Person) RETURN DISTINCT v.city";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH with DISTINCT parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== OPTIONAL MATCH Execution Tests ====================

#[test]
fn test_optional_match_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .query("OPTIONAL MATCH (v:Person) RETURN v.name")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_optional_match_execution_with_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("OPTIONAL MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name")
        .assert_success()
        .assert_result_count(3);
}

// ==================== MATCH Variable-Length Path Semantics Tests ====================

/// First-column values of the most recent result, in emission order.
fn first_column(scenario: &TestScenario) -> Vec<Value> {
    match scenario.last_result().expect("query produced a result") {
        ExecutionResult::DataSet { data, .. } => data
            .rows
            .iter()
            .map(|row| row.first().cloned().expect("result row has a column"))
            .collect(),
        other => panic!("expected a data set, got {other:?}"),
    }
}

/// Name and edge endpoint pair of a `RETURN b.name, e` row.
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

/// The direct hop 1 -> 4 costs 10 while the detour costs 0.5 per hop, so
/// accumulated cost order disagrees with hop-count order.
fn weighted_semantic_graph() -> TestScenario {
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

#[test]
fn test_match_weighted_shortest_walks_until_frontier_exhausted() {
    // A bare `*WEIGHTED(prop)` carries no hop bound: the walk must keep
    // expanding until the frontier is exhausted instead of stopping after
    // the first hop. The direct hop costs 10 while the detour costs 0.5 per
    // hop, so David can only be reached through the cheap 3 -> 4 hop.
    let scenario = weighted_semantic_graph()
        .query(
            "MATCH (a:Person)-[e:KNOWS*WEIGHTED(w)]->(b:Person) \
             WHERE a.name == 'Alice' RETURN b.name, e",
        )
        .assert_success();
    let result = scenario.last_result().expect("query produced a result");
    let data = match result {
        ExecutionResult::DataSet { data, .. } => data,
        other => panic!("expected a data set, got {other:?}"),
    };
    let names = first_column(&scenario);
    assert!(
        names.contains(&Value::string("Charlie")),
        "a two-hop destination must be reachable: {names:?}"
    );
    let david_hops: Vec<(i64, i64)> = data
        .rows
        .iter()
        .filter_map(|row| match row_name_and_edge(row) {
            (Some(name), Some(edge)) if name == "David" => Some(edge),
            _ => None,
        })
        .collect();
    assert_eq!(
        david_hops,
        vec![(3, 4)],
        "the expensive direct hop must not win: David is reached via 3 -> 4"
    );
}

#[test]
fn test_match_unbounded_star_walks_multiple_hops() {
    // A bare `*` carries no hop bound: the walk must keep expanding until
    // the frontier is exhausted instead of stopping after the first hop.
    // 1 -> 4 is a direct hop and 1 -> 2 -> 3 -> 4 a three-hop detour, so
    // David is reachable twice. Without an explicit semantic the unbounded
    // enumeration uses the trail repeat rule, the weakest one that stays
    // finite on a cyclic graph.
    let scenario = weighted_semantic_graph()
        .query(
            "MATCH (a:Person)-[:KNOWS*]->(b:Person) \
             WHERE a.name == 'Alice' RETURN b.name",
        )
        .assert_success();
    let mut names: Vec<String> = first_column(&scenario)
        .iter()
        .map(|value| match value {
            Value::String(s) => s.to_string(),
            other => panic!("expected a name string, got {other:?}"),
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["Bob", "Charlie", "David", "David"],
        "the unbounded hop count must reach multi-hop destinations"
    );
}

#[test]
fn test_match_bounded_star_stays_a_union_of_depths() {
    // A bounded bare `*` stays a union of the listed depths: hop 1 reaches
    // Bob and David, hop 2 reaches Charlie, and the detour to David needs
    // three hops, so it is not part of this range.
    let scenario = weighted_semantic_graph()
        .query(
            "MATCH (a:Person)-[:KNOWS*1..2]->(b:Person) \
             WHERE a.name == 'Alice' RETURN b.name",
        )
        .assert_success();
    let mut names: Vec<String> = first_column(&scenario)
        .iter()
        .map(|value| match value {
            Value::String(s) => s.to_string(),
            other => panic!("expected a name string, got {other:?}"),
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["Bob", "Charlie", "David"]);
}

/// A triangle plus a tail: 1 -> 2 -> 3 -> 1 closes the cycle and 3 -> 4
/// leaves it.
fn cyclic_semantic_graph() -> TestScenario {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE) FROM Person TO Person")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 3:('2020-02-01'), 3 -> 1:('2020-03-01'), 3 -> 4:('2020-04-01')")
        .assert_success()
}

#[test]
fn test_match_trail_walks_until_frontier_exhausted() {
    // `*TRAIL` forbids repeating an edge but permits repeating a vertex, so
    // the cycle 1 -> 2 -> 3 -> 1 (three distinct edges) may return the walk
    // to its own start.
    let scenario = cyclic_semantic_graph()
        .query("MATCH (a:Person)-[:KNOWS*TRAIL]->(b:Person) WHERE a.name == 'Alice' RETURN b.name")
        .assert_success();
    let names = first_column(&scenario);
    assert!(
        names.contains(&Value::string("Charlie")),
        "a two-hop destination must be reachable: {names:?}"
    );
    assert!(
        names.contains(&Value::string("Alice")),
        "*TRAIL only forbids repeated edges, so the cycle may return to the start: {names:?}"
    );
}

#[test]
fn test_match_acyclic_walks_multiple_hops() {
    // `*ACYCLIC` forbids repeating a vertex, so the cycle cannot carry the
    // walk back to its own start.
    let scenario = cyclic_semantic_graph()
        .query(
            "MATCH (a:Person)-[:KNOWS*ACYCLIC]->(b:Person) WHERE a.name == 'Alice' RETURN b.name",
        )
        .assert_success();
    let names = first_column(&scenario);
    assert!(
        names.contains(&Value::string("Charlie")),
        "a two-hop destination must be reachable: {names:?}"
    );
    assert!(
        !names.contains(&Value::string("Alice")),
        "*ACYCLIC forbids repeated vertices, so the cycle cannot return to the start: {names:?}"
    );
}
