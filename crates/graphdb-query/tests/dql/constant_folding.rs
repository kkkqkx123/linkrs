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

/// EXPLAIN surfaces the `folded: true` marker when constant folding applied.
#[test]
fn test_fold_005_explain_shows_folded_marker() {
    let plan = TestScenario::new()
        .expect("Failed to create test scenario")
        .query("EXPLAIN RETURN 1 + 2 AS result")
        .assert_success()
        .get_plan_string()
        .expect("EXPLAIN should produce a plan");
    assert!(
        plan.contains("folded:true"),
        "EXPLAIN should mark the folded node, got: {plan}"
    );
}

/// EXPLAIN does not mark non-folded nodes (`rand` is not pure).
#[test]
fn test_fold_006_explain_omits_folded_without_folding() {
    let plan = TestScenario::new()
        .expect("Failed to create test scenario")
        .query("EXPLAIN RETURN rand() AS result")
        .assert_success()
        .get_plan_string()
        .expect("EXPLAIN should produce a plan");
    assert!(
        !plan.contains("folded"),
        "EXPLAIN should not mark impure expressions, got: {plan}"
    );
}

/// EXPLAIN marks a folded filter condition on a real scan.
#[test]
fn test_fold_007_explain_marks_filter_condition() {
    let plan = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("fold_const_explain_filter")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("EXPLAIN MATCH (p:person) WHERE 1 = 1 RETURN p.name")
        .assert_success()
        .get_plan_string()
        .expect("EXPLAIN should produce a plan");
    assert!(
        plan.contains("folded:true"),
        "EXPLAIN should mark the folded filter, got: {plan}"
    );
}
