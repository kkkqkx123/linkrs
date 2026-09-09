//! Macro DDL Integration Tests
//!
//! Test coverage:
//! - CREATE MACRO with parameters and defaults
//! - SHOW MACROS returns a macro list
//! - Macro expansion in query
//! - DROP MACRO
//! - Error cases: duplicate creation, missing parameters, recursive macro

use super::common;

use common::test_scenario::TestScenario;
use graphdb_core::Value;

// ==================== Basic Macro Lifecycle ====================

/// TC-MACRO-BASIC-001: Create a macro, show it, use it, drop it
#[test]
fn test_macro_basic_lifecycle() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO double(x) AS x * 2")
        .assert_success()
        .query("SHOW MACROS")
        .assert_result_contains(vec![Value::string("double")])
        .query("RETURN double(21)")
        .assert_result_contains(vec![Value::Int(42)])
        .exec_ddl("DROP MACRO double")
        .assert_success()
        .query("SHOW MACROS")
        .assert_result_count(0);
}

/// TC-MACRO-BASIC-002: Macro with default parameter values
#[test]
fn test_macro_default_parameters() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO plus(x, y = 10) AS x + y")
        .assert_success()
        .query("RETURN plus(5)")
        .assert_result_contains(vec![Value::Int(15)])
        .query("RETURN plus(5, 7)")
        .assert_result_contains(vec![Value::Int(12)]);
}

/// TC-MACRO-BASIC-003: Multiple parameters with mixed defaults
#[test]
fn test_macro_multiple_defaults() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO prod(a, b = 2, c = 1) AS a * b * c")
        .assert_success()
        .query("RETURN prod(3)")
        .assert_result_contains(vec![Value::Int(6)])
        .query("RETURN prod(3, 4)")
        .assert_result_contains(vec![Value::Int(12)])
        .query("RETURN prod(3, 4, 5)")
        .assert_result_contains(vec![Value::Int(60)]);
}

// ==================== Macro Error Cases ====================

/// TC-MACRO-ERROR-001: Cannot create duplicate macro without IF NOT EXISTS
#[test]
fn test_macro_duplicate_creation_fails() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO foo(x) AS x + 1")
        .assert_success()
        .exec_ddl("CREATE MACRO foo(y) AS y * 2");

    let error = scenario.error().expect("Duplicate macro creation should fail");
    assert!(error.contains("already exists"), "error: {error}");
}

/// TC-MACRO-ERROR-002: IF NOT EXISTS tolerates a duplicate
#[test]
fn test_macro_if_not_exists_tolerates_duplicate() {
    // IF NOT EXISTS must not fail when the macro already exists.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO foo(x) AS x + 1")
        .assert_success()
        .exec_ddl("CREATE MACRO IF NOT EXISTS foo(y) AS y * 2")
        .assert_success();
}

/// TC-MACRO-ERROR-003: Drop of a non-existent macro without IF EXISTS fails
#[test]
fn test_drop_nonexistent_macro_fails() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP MACRO does_not_exist");

    let error = scenario.error().expect("DROP on nonexistent should fail");
    assert!(error.contains("does not exist"), "error: {error}");
}

/// TC-MACRO-ERROR-004: IF EXISTS tolerates a drop of a non-existent macro
#[test]
fn test_drop_nonexistent_if_exists_ok() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP MACRO IF EXISTS does_not_exist")
        .assert_success();
}

/// TC-MACRO-ERROR-005: Missing required parameter (no default) fails
#[test]
fn test_macro_missing_parameter_fails() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO plus(a, b) AS a + b")
        .assert_success()
        .query("RETURN plus(5)");

    let error = scenario.error().expect("Missing param should fail");
    assert!(
        error.contains("missing") && (error.contains("argument") || error.contains("parameter")),
        "error: {error}"
    );
}

/// TC-MACRO-ERROR-006: Too many parameters fails
#[test]
fn test_macro_too_many_parameters_fails() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO plus(a, b) AS a + b")
        .assert_success()
        .query("RETURN plus(1, 2, 3)");

    let error = scenario.error().expect("Too many params should fail");
    assert!(
        error.contains("at most") || error.contains("more parameters"),
        "error: {error}"
    );
}

/// TC-MACRO-ERROR-007: Direct recursion (self-call) is rejected
#[test]
fn test_macro_direct_recursion_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO fact(n) AS n * fact(n - 1)")
        .assert_success()
        .query("RETURN fact(5)");

    let error = scenario.error().expect("Recursive macro should be rejected");
    assert!(
        error.contains("recursive") || error.contains("expansion"),
        "error: {error}"
    );
}

/// TC-MACRO-KEYWORD-001: Type keywords can be used as macro names
#[test]
fn test_type_keyword_as_macro_name_allowed() {
    // Type keywords are allowed in expression position as identifiers and
    // also as macro names in CREATE MACRO.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO int(x) AS x + 1")
        .assert_success()
        .query("RETURN int(41)")
        .assert_result_contains(vec![Value::Int(42)]);
}

/// TC-MACRO-EXPAND-001: Nested macro expansion (different macros) works
#[test]
fn test_nested_macro_expansion() {
    // Two different macros: double(x) = 2*x; quad(x) = double(x*2)
    // This nested should not be rejected as recursive — recursion means self-call.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE MACRO double(x) AS x * 2")
        .assert_success()
        .exec_ddl("CREATE MACRO quad(x) AS double(x * 2)")
        .assert_success()
        .query("RETURN quad(5)")
        .assert_result_contains(vec![Value::Int(20)]);
}