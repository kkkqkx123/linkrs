//! Metadata-count integration tests.
//!
//! Pure `COUNT` queries fold into a single metadata-count source when the
//! aggregate chain is provably equivalent to the seed-driven sum: the
//! aggregate is a global all-`COUNT` with row-counting arguments, the chain
//! between it and the scan is value-only projects, and the storage gates
//! (read-only handle, known tags/types, no TTL) all pass. Every refusal
//! keeps the original plan and the exact same result.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;

fn setup_line() -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("metadata_count_line")
        .exec_ddl("CREATE TAG Node(name STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG Other(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE Link(weight DOUBLE) FROM Node TO Node")
        .assert_success();
    for i in 0..5 {
        scenario = scenario.exec_dml(&format!("INSERT VERTEX Node(name) VALUES {i}:(\"{i}\")"));
    }
    scenario = scenario.assert_success();
    scenario = scenario
        .exec_dml("INSERT VERTEX Other(name) VALUES 100:(\"100\")")
        .assert_success();
    for i in 0..4 {
        scenario = scenario.exec_dml(&format!(
            "INSERT EDGE Link(weight) VALUES {i} -> {}:(1.0)",
            i + 1
        ));
    }
    scenario.assert_success()
}

fn hub_scenario() -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("metadata_count_hub")
        .exec_ddl("CREATE TAG Node(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE Link(weight DOUBLE) FROM Node TO Node")
        .assert_success();
    for i in 0..6 {
        scenario = scenario.exec_dml(&format!("INSERT VERTEX Node(name) VALUES {i}:(\"{i}\")"));
    }
    scenario = scenario.assert_success();
    for dst in 1..6 {
        scenario = scenario.exec_dml(&format!("INSERT EDGE Link(weight) VALUES 0 -> {dst}:(1.0)"));
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

fn assert_marker(plan: &str, query: &str) {
    assert!(
        plan.contains("metadata_count"),
        "pure count must use the metadata path, got:\n{plan}\nfor {query}"
    );
}

fn assert_no_marker(plan: &str, query: &str) {
    assert!(
        !plan.contains("metadata_count"),
        "non-pure count must keep the original plan, got:\n{plan}\nfor {query}"
    );
}

#[test]
fn vertex_count_uses_metadata_with_exact_value() {
    let query = "MATCH (n:Node) RETURN count(n)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 5);
}

#[test]
fn vertex_star_count_uses_metadata() {
    let query = "MATCH (n:Node) RETURN count(*)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 5);
}

#[test]
fn vertex_multi_count_shares_one_metadata_row() {
    let query = "MATCH (n:Node) RETURN count(n), count(*)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    match scenario.last_result() {
        Some(ExecutionResult::DataSet { data, .. }) => {
            assert_eq!(data.rows.len(), 1);
            assert_eq!(data.rows[0].len(), 2);
            for cell in &data.rows[0] {
                assert!(
                    matches!(cell, Value::BigInt(5)),
                    "every count shares the metadata total, got {cell:?}"
                );
            }
        }
        other => panic!("expected a count dataset, got {other:?}"),
    }
}

#[test]
fn edge_count_uses_metadata_with_exact_value() {
    let query = "MATCH ()-[r:Link]->() RETURN count(r)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 4);
}

#[test]
fn edge_count_with_schema_tags_uses_metadata() {
    let query = "MATCH (:Node)-[r:Link]->(:Node) RETURN count(r)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 4);
}

#[test]
fn edge_seed_count_falls_back_with_exact_value() {
    // Seed-variable counts never enter the optimizer's count-only tail
    // (it only proves edge/destination counts), so metadata refuses and
    // the row path counts the non-null seeds exactly.
    let query = "MATCH (a:Node)-[r:Link]->(b:Node) RETURN count(a)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_no_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 4);
}

#[test]
fn edge_count_mismatched_seed_tag_falls_back_with_zero() {
    // The seed scan covers `Other`, which owns no `Link` out-edges: the
    // schema gate refuses metadata and the degree sum observes zero.
    let query = "MATCH (:Other)-[r:Link]->(:Node) RETURN count(r)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_no_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 0);
}

#[test]
fn vertex_where_filter_falls_back_with_exact_value() {
    let query = "MATCH (n:Node) WHERE n.name > \"2\" RETURN count(n)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_no_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 2);
}

#[test]
fn edge_star_count_uses_metadata() {
    let query = "MATCH ()-[r:Link]->() RETURN count(*)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 4);
}

#[test]
fn vertex_grouped_count_falls_back_with_exact_groups() {
    let query = "MATCH (n:Node) RETURN n.name, count(n)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_no_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(5);
}

#[test]
fn undirected_edge_count_falls_back() {
    // Both-direction walks are out of scope for metadata: the existing
    // count-only tail sums both directions (8 here, not 4). The refusal
    // keeps that behavior untouched.
    let query = "MATCH (a)-[r:Link]-(b) RETURN count(r)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_no_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    scenario.assert_success().assert_result_count(1);
}

#[test]
fn multi_hop_count_falls_back_with_exact_value() {
    let query = "MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)";
    let scenario = setup_line();
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_no_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 3);
}

#[test]
fn edge_ttl_falls_back_with_exact_value() {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("metadata_count_edge_ttl")
        .exec_ddl("CREATE TAG Node(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE Link(expire_at INT64, ttl_duration=100, ttl_col=expire_at) FROM Node TO Node")
        .assert_success();
    for i in 0..3 {
        scenario = scenario.exec_dml(&format!("INSERT VERTEX Node(name) VALUES {i}:(\"{i}\")"));
    }
    scenario = scenario.assert_success();
    scenario = scenario
        .exec_dml("INSERT EDGE Link(expire_at) VALUES 0 -> 1:(9999999999)")
        .assert_success();
    scenario = scenario
        .exec_dml("INSERT EDGE Link(expire_at) VALUES 1 -> 2:(0)")
        .assert_success();

    let query = "MATCH ()-[r:Link]->() RETURN count(r)";
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_no_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 1);
}

#[test]
fn vertex_ttl_falls_back_matching_cursor() {
    // The streaming scan counts TTL-expired vertices (it never decodes
    // expiry); metadata refuses TTL tags so the fallback observes the
    // same rows, keeping both paths consistent today and after a future
    // cursor TTL fix.
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("metadata_count_vertex_ttl")
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

    let query = "MATCH (n:Node) RETURN count(n)";
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_no_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();

    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 2);
}

#[test]
fn delete_vertex_then_counts_stay_exact() {
    // Plain vertex deletes leave incident edges behind. The metadata edge
    // walk excludes edges whose seed-side endpoint is gone (like the
    // degree sum) while keeping edges with a missing far side.
    let scenario = hub_scenario();
    let scenario = scenario
        .exec_dml("DELETE VERTEX Node FROM 0")
        .assert_success();

    let edge_query = "MATCH ()-[r:Link]->() RETURN count(r)";
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {edge_query}"));
    assert_marker(&plan_of(&scenario), edge_query);
    let scenario = scenario.assert_success();
    let scenario = scenario.query(edge_query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(
        count_value(&scenario),
        0,
        "edges out of the deleted seed vanish from the count"
    );

    let vertex_query = "MATCH (n:Node) RETURN count(n)";
    let scenario = scenario.query(vertex_query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(count_value(&scenario), 5);
}

#[test]
fn delete_destination_keeps_dangling_edge_counted() {
    // The count path sums seed-side degrees without checking far-side
    // existence, so an edge into a deleted vertex still counts.
    let scenario = hub_scenario();
    let scenario = scenario
        .exec_dml("DELETE VERTEX Node FROM 1")
        .assert_success();

    let query = "MATCH ()-[r:Link]->() RETURN count(r)";
    let scenario = scenario.query(&format!("EXPLAIN FORMAT = DOT {query}"));
    assert_marker(&plan_of(&scenario), query);
    let scenario = scenario.assert_success();
    let scenario = scenario.query(query);
    let scenario = scenario.assert_success().assert_result_count(1);
    assert_eq!(
        count_value(&scenario),
        5,
        "dangling far-side edges count like the degree sum"
    );
}
