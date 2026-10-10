//! Scan edge identity integration tests.
//!
//! Covers the `identity_only` edge scan annotation end to end: EXPLAIN
//! surfaces `mode:identity` exactly when the scan variable is never
//! consumed as a whole entity, and query results are identical with the
//! annotation on. Boxed edges still flow untouched when the annotation is
//! blocked.
//!
//! Edge scans reach the executor through `LOOKUP ON <edge>` (MATCH plans
//! lower edge patterns to expands in this cost regime), so the tests use
//! the lookup vehicle throughout.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;

fn setup() -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("scan_edge_identity")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE rated(weight INT)")
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
        scenario = scenario.exec_dml(&format!(
            "INSERT EDGE rated(weight) VALUES {i} -> {}:({})",
            i + 1,
            i * 10
        ));
    }
    scenario.assert_success()
}

fn plan_of(scenario: &TestScenario) -> String {
    scenario.get_plan_string().unwrap_or_default()
}

#[test]
fn edge_identity_bare_lookup_keeps_boxed_edges_with_properties() {
    // A bare lookup exposes the entity column directly to the client, so
    // the annotation must stay off and every property must be decoded.
    let scenario = setup();
    let scenario = scenario.query("EXPLAIN FORMAT = DOT LOOKUP ON rated");
    assert!(
        !plan_of(&scenario).contains("mode: identity"),
        "root-level edge scan must keep boxed edges"
    );
    let scenario = scenario.assert_success();

    let scenario = scenario.query("LOOKUP ON rated");
    let scenario = scenario.assert_success().assert_result_count(19);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert!(
                data.rows
                    .iter()
                    .all(|row| matches!(row.first(), Some(Value::Edge(_)))),
                "entity column must hold boxed edges"
            );
            let mut weights: Vec<i32> = data
                .rows
                .iter()
                .filter_map(|row| match row.first() {
                    Some(Value::Edge(edge)) => match edge.props.get("weight") {
                        Some(Value::Int(w)) => Some(*w),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            weights.sort_unstable();
            let expected: Vec<i32> = (0..19).map(|i| i * 10).collect();
            assert_eq!(weights, expected, "boxed edges must carry all properties");
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}

#[test]
fn edge_identity_yield_flat_projection_annotated_with_exact_results() {
    let scenario = setup();
    let scenario = scenario.query("EXPLAIN FORMAT = DOT LOOKUP ON rated YIELD rated.weight");
    let plan = plan_of(&scenario);
    assert!(
        plan.contains("mode: identity"),
        "flat-only edge scan must use identity mode, got:\n{plan}"
    );
    let scenario = scenario.assert_success();

    // Exact results through the identity path: 19 weights.
    let scenario = scenario.query("LOOKUP ON rated YIELD rated.weight");
    let scenario = scenario.assert_success().assert_result_count(19);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert_eq!(data.rows.first(), Some(&vec![Value::Int(0)]));
            assert_eq!(data.rows.last(), Some(&vec![Value::Int(180)]));
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}

#[test]
fn edge_identity_sorted_flat_projection_exact() {
    // The identity scan emits the header column alongside the rows; the
    // downstream flow must return exact results with it attached.
    let scenario = setup();
    let scenario = scenario.query("LOOKUP ON rated YIELD rated.weight ORDER BY rated.weight");
    let scenario = scenario.assert_success().assert_result_count(19);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            let weights: Vec<i32> = data
                .rows
                .iter()
                .filter_map(|row| match row.first() {
                    Some(Value::Int(w)) => Some(*w),
                    _ => None,
                })
                .collect();
            let expected: Vec<i32> = (0..19).map(|i| i * 10).collect();
            assert_eq!(weights, expected, "sorted weights must be exact");
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}

#[test]
fn edge_identity_filtered_flat_projection_exact() {
    // Weights above 50 are rows 6..19 (13 edges).
    let scenario = setup();
    let scenario = scenario.query("LOOKUP ON rated WHERE rated.weight > 50 YIELD rated.weight");
    let scenario = scenario.assert_success().assert_result_count(13);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            let weights: Vec<i32> = data
                .rows
                .iter()
                .filter_map(|row| match row.first() {
                    Some(Value::Int(w)) => Some(*w),
                    _ => None,
                })
                .collect();
            let expected: Vec<i32> = (6..19).map(|i| i * 10).collect();
            assert_eq!(weights, expected, "filtered weights must be exact");
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}

#[test]
fn edge_identity_blocked_for_entity_function() {
    let scenario = setup();
    let scenario = scenario.query("EXPLAIN FORMAT = DOT LOOKUP ON rated YIELD src(rated)");
    assert!(
        !plan_of(&scenario).contains("mode: identity"),
        "entity function argument must keep boxed edges"
    );
    let scenario = scenario.assert_success();

    // The boxed path still evaluates the function exactly.
    let scenario = scenario.query("LOOKUP ON rated YIELD src(rated)");
    let scenario = scenario.assert_success().assert_result_count(19);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert_eq!(data.rows.first(), Some(&vec![Value::BigInt(0)]));
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}
