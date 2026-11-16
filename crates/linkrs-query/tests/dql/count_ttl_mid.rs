//! TTL intermediate counting: pure counts over TTL tables refuse metadata
//! and count through the TTL-filtering cursor path with minimal decode.
//!
//! The scan fixes its time basis once per scan, so the five read shapes
//! (materialized, cursor batch, cursor flat, columnar batch, global count)
//! observe the same live rows. This file pins the query-visible end of that
//! contract: metadata stays refused, expired rows stay invisible.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;

fn plan_of(scenario: &TestScenario) -> String {
    scenario.get_plan_string().unwrap_or_default()
}

fn count_value(scenario: &TestScenario) -> i64 {
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => match data.rows.first() {
            Some(row) => match row.first() {
                Some(Value::BigInt(v)) => *v,
                Some(Value::Int(v)) => *v as i64,
                other => panic!("expected integer count, got {other:?}"),
            },
            None => panic!("expected one count row"),
        },
        other => panic!("expected a count dataset, got {other:?}"),
    }
}

fn vertex_ttl_scenario() -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("count_ttl_mid_vertex")
        .exec_ddl(
            "CREATE TAG Node(name STRING, expire_at INT64, ttl_duration=100, ttl_col=expire_at)",
        )
        .assert_success();
    scenario = scenario
        .exec_dml("INSERT VERTEX Node(name, expire_at) VALUES 0:(\"0\", 9999999999)")
        .assert_success();
    scenario = scenario
        .exec_dml("INSERT VERTEX Node(name, expire_at) VALUES 1:(\"1\", 0)")
        .assert_success();
    scenario = scenario
        .exec_dml("INSERT VERTEX Node(name, expire_at) VALUES 2:(\"2\", 9999999999)")
        .assert_success();
    scenario
}

#[test]
fn ttl_vertex_pure_count_refuses_metadata_and_pins_live() {
    let query = "MATCH (n:Node) RETURN count(n)";
    let scenario = vertex_ttl_scenario();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    let plan = plan_of(&scenario);
    assert!(
        !plan.contains("metadata_count"),
        "TTL tables refuse metadata, got:\n{plan}"
    );
    let scenario = scenario.assert_success();
    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 2);
}

#[test]
fn ttl_vertex_filtered_count_pins_live() {
    let query = "MATCH (n:Node) WHERE n.name == \"0\" RETURN count(n)";
    let scenario = vertex_ttl_scenario();
    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 1);
}

#[test]
fn ttl_vertex_all_expired_counts_zero() {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("count_ttl_mid_all_dead")
        .exec_ddl(
            "CREATE TAG Node(name STRING, expire_at INT64, ttl_duration=100, ttl_col=expire_at)",
        )
        .assert_success();
    scenario = scenario
        .exec_dml("INSERT VERTEX Node(name, expire_at) VALUES 0:(\"0\", 0)")
        .assert_success();
    let query = "MATCH (n:Node) RETURN count(n)";
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert!(!plan_of(&scenario).contains("metadata_count"));
    let scenario = scenario.assert_success();
    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 0);
}
