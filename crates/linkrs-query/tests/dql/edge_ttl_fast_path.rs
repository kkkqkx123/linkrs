//! Edge-TTL fast-path degradation integration tests.
//!
//! The topology fast paths (multi-hop frontier intermediate hops, the fixed
//! `*k` range frontier, and the degree-count tail) read raw CSR adjacency and
//! cannot evaluate TTL. When any reachable edge type has TTL they degrade to
//! the materializing path, so an expired edge behaves exactly like a missing
//! one. The non-TTL control pins the unfiltered shape to prove the exclusion
//! is caused by TTL, not by the query.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;

/// Graph on `edge DDL`: vertex 0 has a fresh `0 -> 1` and an expired
/// `0 -> 4`; `1 -> 3` and `4 -> 2` are fresh. The live 2-hop path reaches
/// vertex 3; the `0 -> 4 -> 2` chain exists only if the expired `0 -> 4` is
/// (incorrectly) crossed by an identifier-only intermediate hop.
fn fast_path_graph(edge_ddl: &str) -> TestScenario {
    let mut scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("ttl_fast_path")
        .exec_ddl("CREATE TAG Node(name STRING)")
        .assert_success()
        .exec_ddl(edge_ddl)
        .assert_success();
    for i in 0..5 {
        scenario = scenario.exec_dml(&format!("INSERT VERTEX Node(name) VALUES {i}:(\"{i}\")"));
    }
    scenario = scenario.assert_success();
    // Fresh edges (far-future ttl column value).
    scenario = scenario
        .exec_dml("INSERT EDGE Link(expire_at) VALUES 0 -> 1:(9999999999)")
        .assert_success();
    scenario = scenario
        .exec_dml("INSERT EDGE Link(expire_at) VALUES 1 -> 3:(9999999999)")
        .assert_success();
    scenario = scenario
        .exec_dml("INSERT EDGE Link(expire_at) VALUES 4 -> 2:(9999999999)")
        .assert_success();
    // Expired edge crossed by the intermediate hop of a 2-hop traversal.
    scenario = scenario
        .exec_dml("INSERT EDGE Link(expire_at) VALUES 0 -> 4:(0)")
        .assert_success();
    scenario
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

const TTL_EDGE_DDL: &str =
    "CREATE EDGE Link(expire_at INT64, ttl_duration=100, ttl_col=expire_at) FROM Node TO Node";
const PLAIN_EDGE_DDL: &str = "CREATE EDGE Link(expire_at INT64) FROM Node TO Node";

#[test]
fn two_hop_frontier_excludes_expired_intermediate_edge() {
    let scenario = fast_path_graph(TTL_EDGE_DDL)
        .query("MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)")
        .assert_success();
    assert_eq!(
        count_value(&scenario),
        1,
        "only the live 0->1->3 path survives; the expired 0->4 must not be crossed"
    );
}

#[test]
fn two_hop_frontier_excludes_expired_without_ttl_is_unfiltered() {
    // Control: the same topology without TTL is unfiltered, proving the
    // exclusion above is driven by TTL and non-TTL behavior is unchanged.
    let scenario = fast_path_graph(PLAIN_EDGE_DDL)
        .query("MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)")
        .assert_success();
    assert_eq!(count_value(&scenario), 2);
}

#[test]
fn fixed_range_frontier_excludes_expired_intermediate_edge() {
    let scenario = fast_path_graph(TTL_EDGE_DDL)
        .query("MATCH (a:Node)-[:Link*2]->(c:Node) RETURN count(c)")
        .assert_success();
    assert_eq!(
        count_value(&scenario),
        1,
        "the fixed 2-hop range must honor the expired 0->4 edge"
    );
}

#[test]
fn degree_count_tail_excludes_expired_edges() {
    // The count-only single-step tail uses the batched degree read, which is
    // also TTL-blind without degradation.
    let scenario = fast_path_graph(TTL_EDGE_DDL)
        .query("MATCH (a:Node)-[:Link]->(b:Node) RETURN count(b)")
        .assert_success();
    assert_eq!(
        count_value(&scenario),
        3,
        "live edges are 0->1, 1->3, 4->2; the expired 0->4 must be excluded"
    );
}

#[test]
fn degree_count_tail_without_ttl_is_unfiltered() {
    let scenario = fast_path_graph(PLAIN_EDGE_DDL)
        .query("MATCH (a:Node)-[:Link]->(b:Node) RETURN count(b)")
        .assert_success();
    assert_eq!(count_value(&scenario), 4);
}
