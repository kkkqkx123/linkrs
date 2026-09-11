//! ATTACH / DETACH DATABASE and CREATE GRAPH / USE GRAPH end-to-end tests.
//!
//! These statements exercise the standard pipeline (parse → bind → plan →
//! execute) rather than any session-level shortcut, so they behave the same
//! for the embedded API and the network server.

use super::common;

use common::test_scenario::TestScenario;
use graphdb_core::Value;
use graphdb_query::attached::clear_attached_databases;

#[test]
fn test_attach_then_show_lists_alias() {
    clear_attached_databases();
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("attach_space")
        .query("ATTACH '/tmp/analytics' AS analytics (DBTYPE KUZU)")
        .assert_success()
        .query("SHOW ATTACHED DATABASES")
        .assert_result_contains(vec![
            Value::string("analytics"),
            Value::string("/tmp/analytics"),
            Value::string("KUZU"),
        ]);
    clear_attached_databases();
}

#[test]
fn test_detach_removes_attachment() {
    clear_attached_databases();
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("attach_space")
        .query("ATTACH '/tmp/src' AS src")
        .assert_success()
        .query("DETACH src")
        .assert_success()
        .query("SHOW ATTACHED DATABASES")
        .assert_result_count(0);
    clear_attached_databases();
}

#[test]
fn test_duplicate_attach_reports_error() {
    clear_attached_databases();
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("attach_space")
        .query("ATTACH '/tmp/a' AS dup")
        .assert_success()
        .query("ATTACH '/tmp/b' AS dup")
        .assert_error();
    clear_attached_databases();
}

#[test]
fn test_detach_unknown_alias_reports_error() {
    clear_attached_databases();
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("attach_space")
        .query("DETACH missing")
        .assert_error();
    clear_attached_databases();
}

#[test]
fn test_create_graph_then_use_graph() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("graph_space_a")
        .query("CREATE GRAPH graph_space_b")
        .assert_success()
        .query("USE GRAPH graph_space_b")
        .assert_success();
}

#[test]
fn test_qualified_node_match_on_attached_alias_reports_catalog_only() {
    clear_attached_databases();
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("attach_space")
        .query("ATTACH '/tmp/analytics' AS analytics")
        .assert_success()
        .query("MATCH (n:analytics.Person) RETURN n");
    let err = scenario.error().unwrap_or_default().to_string();
    assert!(
        err.contains("catalog-only"),
        "Expected catalog-only error, got: {err}"
    );
    clear_attached_databases();
}

#[test]
fn test_qualified_edge_match_on_attached_alias_reports_catalog_only() {
    clear_attached_databases();
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("attach_space")
        .query("ATTACH '/tmp/analytics' AS analytics")
        .assert_success()
        .query("MATCH (a)-[e:analytics.KNOWS]->(b) RETURN e");
    let err = scenario.error().unwrap_or_default().to_string();
    assert!(
        err.contains("catalog-only"),
        "Expected catalog-only error, got: {err}"
    );
    clear_attached_databases();
}

#[test]
fn test_qualified_match_on_unknown_alias_reports_unsupported() {
    clear_attached_databases();
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("attach_space")
        .query("MATCH (n:ghost.Person) RETURN n");
    let err = scenario.error().unwrap_or_default().to_string();
    assert!(
        err.contains("not supported"),
        "Expected unsupported-qualified-name error, got: {err}"
    );
    clear_attached_databases();
}
