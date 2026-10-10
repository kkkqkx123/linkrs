//! Scan identity integration tests.
//!
//! Covers the `identity_only` scan annotation end to end: EXPLAIN surfaces
//! `mode:identity` exactly when the scan variable is never consumed as a
//! whole entity, and query results are identical with the annotation on.
//! The count-only `unit:chunks` estimate annotation is covered alongside,
//! since the same queries exercise it.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;

fn setup() -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("scan_identity")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE knows()")
        .assert_success();
    for i in 0..20 {
        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX person(name, age) VALUES {}:(\"{}\", {})",
            i,
            i,
            20 + i
        ));
    }
    scenario = scenario.assert_success();
    for i in 0..19 {
        scenario = scenario.exec_dml(&format!("INSERT EDGE knows VALUES {i} -> {j}", j = i + 1));
    }
    scenario.assert_success()
}

fn plan_of(scenario: &TestScenario) -> String {
    scenario.get_plan_string().unwrap_or_default()
}

fn first_string_cell(scenario: &TestScenario) -> Option<String> {
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => data
            .rows
            .first()
            .and_then(|row| row.first())
            .map(|value| format!("{value:?}")),
        _ => None,
    }
}

#[test]
fn identity_flat_projection_annotated_with_exact_results() {
    let scenario = setup();
    let scenario = scenario.query("EXPLAIN FORMAT = DOT MATCH (a:person) RETURN a.name");
    assert!(
        plan_of(&scenario).contains("mode: identity"),
        "flat-only scan must use identity mode"
    );
    let scenario = scenario.assert_success();

    // Exact results through the identity path: 20 names, ordered.
    let scenario = scenario.query("MATCH (a:person) RETURN a.name ORDER BY a.name");
    let scenario = scenario.assert_success().assert_result_count(20);
    assert_eq!(
        first_string_cell(&scenario),
        Some("String(\"0\")".to_string())
    );
}

#[test]
fn identity_filtered_flat_projection_with_exact_results() {
    let scenario = setup();
    let scenario =
        scenario.query("EXPLAIN FORMAT = DOT MATCH (a:person) WHERE a.age > 25 RETURN a.name");
    assert!(
        plan_of(&scenario).contains("mode: identity"),
        "filtered flat scan must use identity mode"
    );
    let scenario = scenario.assert_success();

    // Ages 26..=39: 14 vertices; string order puts "10" first.
    let scenario =
        scenario.query("MATCH (a:person) WHERE a.age > 25 RETURN a.name ORDER BY a.name");
    let scenario = scenario.assert_success().assert_result_count(14);
    assert_eq!(
        first_string_cell(&scenario),
        Some("String(\"10\")".to_string())
    );
}

#[test]
fn identity_seed_only_scan_for_count() {
    let scenario = setup();
    let scenario = scenario
        .query("EXPLAIN FORMAT = DOT MATCH (a:person)-[:knows]->(b:person) RETURN count(b)");
    let plan = plan_of(&scenario);
    assert!(
        plan.contains("mode: identity"),
        "seed-only scan must use identity mode, got:\n{plan}"
    );
    assert!(
        plan.contains("unit: chunks"),
        "count-only expand estimate counts chunks, got:\n{plan}"
    );
    let scenario = scenario.assert_success();

    let scenario = scenario.query("MATCH (a:person)-[:knows]->(b:person) RETURN count(b)");
    let scenario = scenario.assert_success().assert_result_count(1);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert_eq!(data.rows, vec![vec![Value::BigInt(19)]]);
        }
        other => panic!("expected a count dataset, got {other:?}"),
    }
}

#[test]
fn identity_count_bare_variable_exact() {
    // `count(a)` observes only null-ness, so the identity scan feeds the
    // aggregate without evaluating or materializing the argument.
    let scenario = setup();
    let scenario = scenario.query("MATCH (a:person) RETURN count(a)");
    let scenario = scenario.assert_success().assert_result_count(1);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert_eq!(data.rows, vec![vec![Value::BigInt(20)]]);
        }
        other => panic!("expected a count dataset, got {other:?}"),
    }
}

#[test]
fn identity_blocked_for_whole_entity_return() {
    let scenario = setup();
    let scenario = scenario.query("EXPLAIN FORMAT = DOT MATCH (a:person) RETURN a");
    assert!(
        !plan_of(&scenario).contains("mode: identity"),
        "whole-entity return must keep boxed vertices"
    );
    let scenario = scenario.assert_success();

    // Boxed entities still flow to the client untouched.
    let scenario = scenario.query("MATCH (a:person) RETURN a");
    let scenario = scenario.assert_success().assert_result_count(20);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert!(
                data.rows
                    .iter()
                    .all(|row| matches!(row.first(), Some(Value::Vertex(_)))),
                "entity column must hold boxed vertices"
            );
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}

#[test]
fn identity_blocked_for_entity_function() {
    let scenario = setup();
    let scenario = scenario.query("EXPLAIN FORMAT = DOT MATCH (a:person) RETURN labels(a)");
    assert!(
        !plan_of(&scenario).contains("mode: identity"),
        "entity function argument must keep boxed vertices"
    );
    let _ = scenario.assert_success();
}
