//! Cross-statement session semantics.
//!
//! Single-statement coverage cannot catch state-transfer bugs, so these
//! tests compose statements: USE then DDL then DML then DQL in one session,
//! transactional DDL+DML atomicity, and pipe/subquery nesting.

mod common;

use common::data_fixtures::social_network_dataset;
use common::test_scenario::TestScenario;
use linkrs_core::Value;

#[test]
fn test_session_use_ddl_dml_dql_state_transfer() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("session_flow")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since STRING) FROM Person TO Person")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020')")
        .assert_success()
        .query("GO FROM 1 OVER KNOWS")
        .assert_success()
        .assert_result_count(1)
        .query("FETCH PROP ON Person 2 YIELD Person.name AS name")
        .assert_success()
        .assert_vertex_or_edge_has_property("name", Value::string("Bob"))
        .query("MATCH (n:Person) RETURN n.name AS name")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_session_copy_then_insert_visible_together() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("session_copy_insert")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("MATCH (n:Person) RETURN n.name AS name")
        .assert_success()
        .assert_result_contains(vec![Value::string("Alice")])
        .exec_dml("INSERT VERTEX Person(name) VALUES 2:('Bob')")
        .assert_success()
        .query("MATCH (n:Person) RETURN n.name AS name")
        .assert_success()
        .assert_result_count(2)
        .assert_result_contains(vec![Value::string("Bob")]);
}

#[test]
fn test_session_pipe_subquery_nesting() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("session_nesting")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query(
            "MATCH (n:Person) RETURN n.name AS name UNION MATCH (m:Person) RETURN m.name AS name",
        )
        .assert_success()
        .assert_result_contains(vec![Value::string("Alice")])
        .query("WITH 1 AS x RETURN x")
        .assert_success()
        .assert_result_columns(&["x"])
        .assert_result_count(1);
}

#[test]
fn test_session_no_planner_leak_across_categories() {
    let scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("session_no_leak")
        .exec_ddl("CREATE TAG Person(id INT)")
        .assert_success()
        .query("SHOW TAGS")
        .assert_success()
        .query("MATCH (n:Person) RETURN n")
        .assert_success()
        .query("KILL QUERY 1, 2")
        .assert_error();
    let error = scenario.error().expect("KILL QUERY must be rejected");
    assert!(
        !error.contains("No planner for bound statement"),
        "session must never leak the internal planner error: {error}"
    );
}

#[test]
fn test_session_fixture_dataset_shape_is_stable() {
    let (vertices, edges) = social_network_dataset();
    assert_eq!(vertices.len(), 4);
    assert_eq!(edges.len(), 4);
}
