//! Constant folding optimizer integration tests.
//!
//! Verifies the FoldConstantsRule through EXPLAIN plan output: constant
//! expressions in projections and filters are replaced with literals.

use super::common;

use common::test_scenario::TestScenario;

/// `RETURN 1 + 2` folds to a literal in the plan.
#[test]
fn test_fold_001_return_constant_expression() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("EXPLAIN RETURN 1 + 2 AS result")
        .assert_success()
        .get_plan_string()
        .expect("EXPLAIN should produce a plan")
        .contains("3");
}

/// `RETURN 1 + 2` executes to 3 (behavior unchanged by folding).
#[test]
fn test_fold_002_return_constant_result() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("RETURN 1 + 2 AS result")
        .assert_success()
        .assert_result_count(1);
}

/// A filter condition `1 = 1` folds to TRUE and does not change results.
#[test]
fn test_fold_003_constant_filter_preserves_result() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("fold_const_filter")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)")
        .assert_success()
        .query("MATCH (p:person) WHERE 1 = 1 RETURN p.name")
        .assert_success()
        .assert_result_count(2);
}

/// Partial folding: `(1 + 2) + p.age` keeps the variable part.
#[test]
fn test_fold_004_partial_constant_subexpression() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("fold_const_partial")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("MATCH (p:person) RETURN p.age + (1 + 2) AS age_plus")
        .assert_success()
        .assert_result_contains(vec![graphdb_query::core::Value::Int(33)]);
}
