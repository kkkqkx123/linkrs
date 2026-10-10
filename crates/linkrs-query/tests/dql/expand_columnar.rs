//! Expand columnarization integration tests.
//!
//! Covers the three-step expand columnarization design: single-step projected
//! and rowless assembly, fixed multi-hop frontiers, and variable-length
//! union collection. Each shape pins exact results on a small tagged schema
//! plus EXPLAIN markers for the new columnar behavior.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;

fn setup_line() -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("expand_columnar_line")
        .exec_ddl("CREATE TAG Node(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE Link(weight DOUBLE) FROM Node TO Node")
        .assert_success();
    for i in 0..5 {
        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX Node(name) VALUES {i}:(\"{i}\")"
        ));
    }
    scenario = scenario.assert_success();
    for i in 0..4 {
        scenario = scenario.exec_dml(&format!(
            "INSERT EDGE Link(weight) VALUES {i} -> {}:(1.0)",
            i + 1
        ));
    }
    scenario.assert_success()
}

fn setup_hub() -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("expand_columnar_hub")
        .exec_ddl("CREATE TAG Node(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE Link(weight DOUBLE) FROM Node TO Node")
        .assert_success();
    for i in 0..6 {
        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX Node(name) VALUES {i}:(\"{i}\")"
        ));
    }
    scenario = scenario.assert_success();
    for dst in 1..6 {
        scenario = scenario.exec_dml(&format!(
            "INSERT EDGE Link(weight) VALUES 0 -> {dst}:(1.0)"
        ));
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

fn sorted_rows(scenario: &TestScenario) -> Vec<Vec<String>> {
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            let mut rows: Vec<Vec<String>> = data
                .rows
                .iter()
                .map(|row| row.iter().map(|v| format!("{v:?}")).collect())
                .collect();
            rows.sort();
            rows
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}

#[test]
fn narrow_count_matches_edges_with_columnar_marker() {
    let scenario = setup_line();
    let scenario =
        scenario.query("EXPLAIN FORMAT = DOT MATCH ()-[r:Link]->() RETURN count(r)");
    let plan = plan_of(&scenario);
    assert!(
        plan.contains("columnar"),
        "narrow materialized expand must use the columnar path, got:\n{plan}"
    );
    let scenario = scenario.assert_success();

    let scenario = scenario.query("MATCH ()-[r:Link]->() RETURN count(r)");
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 4);
}

#[test]
fn edge_property_projection_exact() {
    let scenario = setup_line();
    let scenario =
        scenario.query("MATCH (a:Node)-[r:Link]->(b:Node) RETURN r.weight ORDER BY r.weight");
    let scenario = scenario.assert_success().assert_result_count(4);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert_eq!(data.rows.len(), 4);
            for row in &data.rows {
                assert!(
                    matches!(row.first(), Some(Value::Double(_))),
                    "edge weight must project exactly, got {row:?}"
                );
            }
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}

#[test]
fn dst_property_projection_exact() {
    let scenario = setup_line();
    let scenario =
        scenario.query("MATCH (a:Node)-[:Link]->(b:Node) RETURN b.name ORDER BY b.name");
    let scenario = scenario.assert_success().assert_result_count(4);
    let rows = sorted_rows(&scenario);
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter().any(|r| r[0].contains("\"1\"")),
        "destination names must project exactly, got {rows:?}"
    );
}

#[test]
fn two_hop_count_matches_chained_shape() {
    let scenario = setup_line();
    let scenario =
        scenario.query("MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)");
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 3);
}

#[test]
fn two_hop_fixed_range_matches_chained_shape() {
    let scenario = setup_line();
    let scenario =
        scenario.query("MATCH (a:Node)-[:Link*2]->(c:Node) RETURN count(c)");
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(
        count_value(&scenario),
        3,
        "fixed 2-hop frontier must match the chained 2-hop fanout"
    );
}

#[test]
fn hub_two_hop_terminates_with_exact_count() {
    let scenario = setup_hub();
    let scenario =
        scenario.query("MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)");
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 0);
    let scenario =
        scenario.query("MATCH (a:Node)-[:Link*2]->(c:Node) RETURN count(c)");
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 0);
}

#[test]
fn variable_length_union_matches_depths() {
    let scenario = setup_line();
    let scenario =
        scenario.query("MATCH (a:Node)-[:Link*1..2]->(c:Node) RETURN count(c)");
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(
        count_value(&scenario),
        7,
        "depths 1..2 from all seeds give 4 + 3"
    );
    let scenario =
        scenario.query("MATCH (a:Node)-[:Link*1..2]->(c:Node) WHERE a.name == \"0\" RETURN c.name ORDER BY c.name");
    let scenario = scenario.assert_success().assert_result_count(2);
    let rows = sorted_rows(&scenario);
    assert_eq!(rows.len(), 2);
    let scenario =
        scenario.query("MATCH (a:Node)-[:Link*1..3]->(c:Node) WHERE a.name == \"0\" RETURN c.name ORDER BY c.name");
    let scenario = scenario.assert_success().assert_result_count(3);
    assert_eq!(sorted_rows(&scenario).len(), 3);
}
