//! Sequence DDL Integration Tests
//!
//! Test coverage:
//! - CREATE SEQUENCE with START / INCREMENT / bounds / CYCLE
//! - next_val / curr_val (and the `nextval` / `currval` aliases) stepping
//! - ALTER SEQUENCE adjusts step and bounds without resetting the value
//! - DROP SEQUENCE with IF EXISTS
//! - Error cases: duplicate creation, missing sequence, INCREMENT=0,
//!   inverted bounds

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;

// ==================== Create + next_val stepping ====================

#[test]
fn test_create_sequence_nextval_steps() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE s START=1 INCREMENT=2")
        .assert_success()
        .query("RETURN nextval('s')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(3)])
        .query("RETURN nextval('s')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(5)]);
}

#[test]
fn test_sequence_next_val_and_curr_val_spellings() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE spelled START=10 INCREMENT=5")
        .assert_success()
        .query("RETURN next_val('spelled')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(15)])
        .query("RETURN curr_val('spelled')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(15)])
        .query("RETURN currval('spelled')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(15)])
        .query("RETURN next_val('spelled')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(20)]);
}

#[test]
fn test_create_sequence_defaults_start_at_one() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE plain_seq")
        .assert_success()
        .query("RETURN curr_val('plain_seq')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(1)])
        .query("RETURN next_val('plain_seq')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(2)]);
}

// ==================== IF NOT EXISTS / duplicate ====================

#[test]
fn test_create_sequence_duplicate_fails() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE dup_seq")
        .assert_success()
        .exec_ddl("CREATE SEQUENCE dup_seq");

    let error = scenario.error().expect("Duplicate sequence should fail");
    assert!(error.contains("already exists"), "error: {error}");
}

#[test]
fn test_create_sequence_if_not_exists_tolerates_duplicate() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE ine_seq START=7")
        .assert_success()
        .exec_ddl("CREATE SEQUENCE IF NOT EXISTS ine_seq START=99")
        .assert_success()
        .query("RETURN curr_val('ine_seq')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(7)]);
}

// ==================== ALTER SEQUENCE ====================

#[test]
fn test_alter_sequence_changes_step_without_reset() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE alt_seq START=0 INCREMENT=1")
        .assert_success()
        .query("RETURN nextval('alt_seq')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(1)])
        .exec_ddl("ALTER SEQUENCE alt_seq INCREMENT=10")
        .assert_success()
        .query("RETURN nextval('alt_seq')")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(11)]);
}

#[test]
fn test_alter_missing_sequence_fails() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("ALTER SEQUENCE no_such_seq INCREMENT=2");

    let error = scenario.error().expect("ALTER on missing should fail");
    assert!(error.contains("does not exist"), "error: {error}");
}

// ==================== DROP SEQUENCE ====================

#[test]
fn test_drop_sequence_lifecycle() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE drop_seq")
        .assert_success()
        .exec_ddl("DROP SEQUENCE drop_seq")
        .assert_success()
        .query("RETURN nextval('drop_seq')");

    let error = scenario.error().expect("nextval after drop should fail");
    assert!(error.contains("does not exist"), "error: {error}");
}

#[test]
fn test_drop_missing_sequence_errors_without_if_exists() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP SEQUENCE no_such_seq");

    let error = scenario.error().expect("DROP on missing should fail");
    assert!(error.contains("does not exist"), "error: {error}");
}

#[test]
fn test_drop_missing_sequence_if_exists_succeeds() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP SEQUENCE IF EXISTS no_such_seq")
        .assert_success();
}

// ==================== Validation ====================

#[test]
fn test_create_sequence_zero_increment_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE stuck_seq INCREMENT=0");

    let error = scenario.error().expect("INCREMENT=0 should fail");
    assert!(error.contains("INCREMENT=0"), "error: {error}");
}

#[test]
fn test_create_sequence_inverted_bounds_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE SEQUENCE bad_bounds MINVALUE=100 MAXVALUE=10");

    let error = scenario.error().expect("inverted bounds should fail");
    assert!(error.contains("MINVALUE"), "error: {error}");
}

#[test]
fn test_nextval_missing_sequence_fails() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("RETURN nextval('ghost_seq')");

    let error = scenario.error().expect("nextval on missing should fail");
    assert!(error.contains("does not exist"), "error: {error}");
}
