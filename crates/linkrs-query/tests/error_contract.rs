//! Error-message contract tests.
//!
//! Each statement category locks one representative error snippet so that
//! user-facing guidance cannot silently regress into internal wording.
//! The internal planner leak is asserted absent on every rejection.

mod common;

use common::test_scenario::TestScenario;

const LEAK: &str = "No planner for bound statement";

fn assert_rejected(sql: &str, needle: &str) {
    let scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("error_contract")
        .query(sql)
        .assert_error();
    let error = scenario.error().expect("statement must fail");
    assert!(
        !error.contains(LEAK),
        "{sql} leaks the internal planner error: {error}"
    );
    assert!(
        error.contains(needle),
        "{sql} error must contain {needle:?}, got: {error}"
    );
}

#[test]
fn test_error_contract_representative_messages() {
    assert_rejected("SET 1.age = 31", "standalone SET is not supported");
    assert_rejected("REMOVE p.age", "standalone REMOVE is not supported");
    assert_rejected(
        "INSTALL EXTENSION my_udf FROM '/tmp/udf_plugin.so'",
        "/v1/functions",
    );
    assert_rejected(
        "MIGRATE PLAN FOR TAG person FROM VERSION 1 TO 2 IN error_contract",
        "/v1/migration/plan",
    );
    assert_rejected("KILL QUERY 1, 2", "network service");
    assert_rejected(
        "LOAD FROM '/nonexistent/linkrs/error_contract.csv'",
        "failed to open",
    );
    assert_rejected("CALL not_a_real_function()", "not_a_real_function");
}

#[test]
fn test_error_contract_feature_gates_point_at_features() {
    #[cfg(not(feature = "fulltext"))]
    assert_rejected("SEARCH INDEX idx MATCH 'x'", "not enabled");
    #[cfg(not(feature = "vector"))]
    assert_rejected("SEARCH VECTOR idx WITH vector=[0.1, 0.2] LIMIT 1", "vector");
}
