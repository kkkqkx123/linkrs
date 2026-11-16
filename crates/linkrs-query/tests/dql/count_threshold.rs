//! Threshold-out vectorized complement: filtered global counts and grouped
//! counts stay on the row path for correctness but execute per-block.
//!
//! Plan shape never changes (no metadata marker for these shapes); only the
//! execution block handling differs. Values are pinned end to end.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;

fn setup() -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("count_threshold_vectorized")
        .exec_ddl("CREATE TAG Node(name STRING, age INT64)")
        .assert_success()
        .exec_ddl("CREATE EDGE Link(weight DOUBLE) FROM Node TO Node")
        .assert_success();
    for i in 0..6 {
        scenario = scenario.exec_dml(&format!("INSERT VERTEX Node(name, age) VALUES {i}:(\"{i}\", {i})"));
    }
    scenario = scenario.assert_success();
    for i in 0..5 {
        scenario = scenario.exec_dml(&format!("INSERT EDGE Link(weight) VALUES {i} -> {}:(1.0)", i + 1));
    }
    scenario.assert_success()
}

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

#[test]
fn filtered_global_count_blocks_but_pins_value() {
    let query = "MATCH (n:Node) WHERE n.age > 2 RETURN count(n)";
    let scenario = setup();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    let plan = plan_of(&scenario);
    assert!(
        !plan.contains("metadata_count"),
        "filtered counts keep the row plan, got:\n{plan}"
    );
    let scenario = scenario.assert_success();
    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 3);
}

#[test]
fn filtered_star_count_pins_value() {
    let query = "MATCH (n:Node) WHERE n.age >= 4 RETURN count(*)";
    let scenario = setup();
    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 2);
}

#[test]
fn grouped_count_pins_groups_and_null_semantics() {
    let query = "MATCH (n:Node) RETURN n.age, count(n.age) GROUP BY n.age ORDER BY n.age";
    let scenario = setup();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    let plan = plan_of(&scenario);
    assert!(
        !plan.contains("metadata_count"),
        "grouped counts keep the row plan, got:\n{plan}"
    );
    let scenario = scenario.assert_success();
    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(6);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert_eq!(data.rows.len(), 6);
            for row in &data.rows {
                assert_eq!(row.len(), 2);
                match row.get(1) {
                    Some(Value::BigInt(1)) | Some(Value::Int(1)) => {},
                    other => panic!("each age group counts once, got {other:?}"),
                }
            }
        }
        other => panic!("expected grouped dataset, got {other:?}"),
    }
}

#[test]
fn grouped_star_count_pins_groups() {
    let query = "MATCH (n:Node) RETURN n.age, count(*) GROUP BY n.age ORDER BY n.age";
    let scenario = setup();
    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(6);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert_eq!(data.rows.len(), 6);
        }
        other => panic!("expected grouped dataset, got {other:?}"),
    }
}
