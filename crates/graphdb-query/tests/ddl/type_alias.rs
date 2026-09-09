//! Type Alias DDL Integration Tests
//!
//! Test coverage:
//! - CREATE TYPE alias AS builtin
//! - CREATE TYPE alias AS another alias (aliasing chain)
//! - CREATE TYPE with cycles (should reject)
//! - DROP TYPE (rejects when referenced by other aliases)
//! - Use type alias in CREATE TAG and CAST

use super::common;

use common::test_scenario::TestScenario;
use graphdb_core::Value;

// ==================== Basic Type Alias ====================

/// TC-TYALIAS-BASIC-001: Alias to builtin type works in CREATE TAG
#[test]
fn test_type_alias_basic_builtin() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE my_int AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE my_float AS FLOAT")
        .assert_success()
        .exec_ddl("CREATE TAG Person(name STRING, age my_int, score my_float)")
        .assert_success()
        // The schema stores the resolved builtin type; aliases resolve at
        // bind time, so DESC reports the underlying (INT/FLOAT).
        .query("DESC TAG Person")
        .assert_result_contains(vec![Value::string("age"), Value::string("INT")])
        .assert_result_contains(vec![Value::string("score"), Value::string("FLOAT")]);
}

/// TC-TYALIAS-BASIC-002: Alias chain resolution works
#[test]
fn test_type_alias_chain_resolution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE a AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE b AS a")
        .assert_success()
        .exec_ddl("CREATE TYPE c AS b")
        .assert_success()
        .exec_ddl("CREATE TAG Data(val c)")
        .assert_success()
        .query("DESC TAG Data")
        .assert_result_contains(vec![Value::string("val"), Value::string("INT")]);
}

/// TC-TYALIAS-BASIC-003: Use alias in CAST (`::`) expression
#[test]
fn test_type_alias_cast_works() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE my_float AS FLOAT")
        .assert_success()
        .query("RETURN 42::my_float")
        .assert_result_contains(vec![Value::Float(42.0)]);
}

// ==================== Cycle Detection ====================

/// TC-TYALIAS-CYCLE-001: Direct self-cycle a → a is rejected
#[test]
fn test_type_alias_direct_cycle_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE a AS a");

    let error = scenario.error().expect("Direct cycle should be rejected");
    assert!(error.contains("Cyclic"), "error: {error}");
}

/// TC-TYALIAS-CYCLE-002: Indirect cycle a→b→a (via forward reference) is rejected
#[test]
fn test_type_alias_indirect_cycle_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        // `a AS b` is a forward reference to a not-yet-defined alias `b`.
        .exec_ddl("CREATE TYPE a AS b")
        .assert_success()
        // Closing the loop by defining `b AS a` must be rejected.
        .exec_ddl("CREATE TYPE b AS a");

    let error = scenario.error().expect("Indirect cycle should be rejected");
    assert!(error.contains("Cyclic"), "error: {error}");
}

/// TC-TYALIAS-CYCLE-003: Three-link cycle a→b→c→a is rejected
#[test]
fn test_type_alias_three_link_cycle_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE a AS c")
        .assert_success()
        .exec_ddl("CREATE TYPE b AS a")
        .assert_success()
        .exec_ddl("CREATE TYPE c AS b");

    let error = scenario.error().expect("Long cycle should be rejected");
    assert!(
        error.contains("Cyclic") || error.contains("already exists"),
        "error: {error}"
    );
}

/// TC-TYALIAS-CYCLE-004: No cycle creation succeeds
#[test]
fn test_type_alias_no_cycle_succeeds() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE a AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE b AS FLOAT")
        .assert_success()
        .exec_ddl("CREATE TYPE c AS STRING")
        .assert_success();
    // No cycles, all accepted
}

// ==================== Drop Dependency Checks ====================

/// TC-TYALIAS-DROP-001: Cannot drop alias referenced by another alias
#[test]
fn test_drop_alias_referenced_by_other_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE base AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE derived AS base")
        .assert_success()
        .exec_ddl("DROP TYPE base");

    let error = scenario.error().expect("DROP on referenced alias should fail");
    assert!(
        error.contains("still referenced") || error.contains("dependency"),
        "error: {error}"
    );
}

/// TC-TYALIAS-DROP-002: Can drop leaf alias (no references)
#[test]
fn test_drop_leaf_alias_succeeds() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE a AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE b AS INT")
        .assert_success()
        .exec_ddl("DROP TYPE b")
        .assert_success();
}

/// TC-TYALIAS-DROP-003: Drop after removing dependency succeeds
#[test]
fn test_drop_after_dependency_removed() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE base AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE derived AS base")
        .assert_success()
        // DROP the dependent first
        .exec_ddl("DROP TYPE derived")
        .assert_success()
        // Now base can be dropped
        .exec_ddl("DROP TYPE base")
        .assert_success();
}

/// TC-TYALIAS-DROP-004: IF EXISTS tolerates drop of non-existent
#[test]
fn test_drop_nonexistent_if_exists_ok() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP TYPE IF EXISTS does_not_exist")
        .assert_success();
}

// ==================== Error Cases ====================

/// TC-TYALIAS-ERROR-001: Duplicate CREATE TYPE fails without IF NOT EXISTS
#[test]
fn test_duplicate_create_fails() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE foo AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE foo AS FLOAT");

    let error = scenario.error().expect("Duplicate creation should fail");
    assert!(error.contains("already exists"), "error: {error}");
}

/// TC-TYALIAS-ERROR-002: IF NOT EXISTS tolerates duplicate
#[test]
fn test_duplicate_if_not_exists_ok() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE foo AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE IF NOT EXISTS foo AS FLOAT")
        .assert_success();
}

// ==================== Integration with Schema ====================

/// TC-TYALIAS-INTG-001: Alias survives INSERT and SELECT
#[test]
fn test_type_alias_insert_select_works() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TYPE user_age AS INT")
        .assert_success()
        .exec_ddl("CREATE TYPE score AS FLOAT")
        .assert_success()
        .exec_ddl("CREATE TAG User(name STRING, age user_age, score score)")
        .assert_success()
        .exec_dml("INSERT VERTEX User(name, age, score) VALUES 1:('Alice', 30, 95.5)")
        .assert_success()
        .query("MATCH (u:User) RETURN u.age, u.score")
        .assert_result_contains(vec![Value::Int(30)])
        .assert_result_contains(vec![Value::Float(95.5)]);
}
