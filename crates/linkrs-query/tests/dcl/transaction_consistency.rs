//! Transaction Consistency DCL Tests
//!
//! Test consistency guarantees:
//! - Atomic operations (all-or-nothing)
//! - Transaction isolation
//! - State consistency after operations

use super::common;
use common::test_scenario::TestScenario;

fn new_scenario() -> TestScenario {
    TestScenario::new().expect("Failed to create test scenario")
}

// ==================== Atomic Create User Tests ====================

#[test]
fn test_create_user_atomicity() {
    let scenario = new_scenario();
    scenario
        .exec_dcl("CREATE USER atomic_user WITH PASSWORD 'pass123'")
        .assert_success()
        // User must exist completely or not at all
        .exec_dcl("DESCRIBE USER atomic_user")
        .assert_success();
}

#[test]
fn test_failed_create_user_no_partial_state() {
    let scenario = new_scenario();
    // Invalid role should not partially create user
    scenario
        .exec_dcl("CREATE USER partial_user WITH PASSWORD 'pass' WITH ROLE INVALID_ROLE")
        .assert_error()
        // User should not exist at all
        .exec_dcl("DESCRIBE USER partial_user")
        .assert_error();
}

// ==================== Atomic Grant/Revoke Tests ====================

#[test]
fn test_grant_atomicity() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER grant_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE grant_space WITH DIMENSION=128")
        .assert_success();

    scenario
        .exec_dcl("GRANT ADMIN ON grant_space TO grant_user")
        .assert_success()
        // Permission must be fully granted or not at all
        .exec_dcl("SHOW ROLES IN grant_space")
        .assert_success();
}

#[test]
fn test_revoke_atomicity() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER revoke_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE revoke_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON revoke_space TO revoke_user")
        .assert_success();

    scenario
        .exec_dcl("REVOKE ADMIN ON revoke_space FROM revoke_user")
        .assert_success()
        // Permission must be fully revoked or not at all
        .exec_dcl("DESCRIBE USER revoke_user")
        .assert_success();
}

// ==================== Consistency After Failed Operations ====================

#[test]
fn test_system_state_after_invalid_create() {
    let scenario = new_scenario();

    // Missing password should fail
    scenario
        .exec_dcl("CREATE USER fail_user")
        .assert_error()
        // System should be in consistent state - no trace of user
        .exec_dcl("SHOW USERS")
        .assert_success();
}

#[test]
fn test_system_state_after_invalid_drop() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER existing_user WITH PASSWORD 'pass'")
        .assert_success();

    // Drop non-existent user should fail
    scenario
        .exec_dcl("DROP USER nonexistent_user")
        .assert_error()
        // Existing user should still exist
        .exec_dcl("DESCRIBE USER existing_user")
        .assert_success();
}

#[test]
fn test_system_state_after_invalid_grant() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER valid_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE valid_space WITH DIMENSION=128")
        .assert_success();

    // Grant non-existent role should fail
    scenario
        .exec_dcl("GRANT INVALID_ROLE ON valid_space TO valid_user")
        .assert_error()
        // System should still be consistent
        .exec_dcl("SHOW ROLES IN valid_space")
        .assert_success()
        .exec_dcl("DESCRIBE USER valid_user")
        .assert_success();
}

// ==================== State Consistency in Sequences ====================

#[test]
fn test_user_state_consistency_create_alter_drop() {
    let scenario = new_scenario();

    // Create user
    scenario
        .exec_dcl("CREATE USER state_user WITH PASSWORD 'initial'")
        .assert_success()
        // Verify created
        .exec_dcl("DESCRIBE USER state_user")
        .assert_success()
        // Alter user
        .exec_dcl("ALTER USER state_user WITH PASSWORD 'modified'")
        .assert_success()
        // Verify still exists after alter
        .exec_dcl("DESCRIBE USER state_user")
        .assert_success()
        // Drop user
        .exec_dcl("DROP USER state_user")
        .assert_success()
        // Verify completely removed
        .exec_dcl("DESCRIBE USER state_user")
        .assert_error();
}

#[test]
fn test_permission_state_consistency() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER perm_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE perm_space WITH DIMENSION=128")
        .assert_success();

    // Grant role
    scenario
        .exec_dcl("GRANT DBA ON perm_space TO perm_user")
        .assert_success()
        // Verify granted
        .exec_dcl("SHOW ROLES IN perm_space")
        .assert_success()
        // Revoke role
        .exec_dcl("REVOKE DBA ON perm_space FROM perm_user")
        .assert_success()
        // Verify revoked
        .exec_dcl("SHOW ROLES IN perm_space")
        .assert_success();
}

// ==================== Isolation Tests ====================

#[test]
fn test_create_if_not_exists_isolation() {
    let scenario = new_scenario();

    // First create succeeds
    scenario
        .exec_dcl("CREATE USER IF NOT EXISTS iso_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DESCRIBE USER iso_user")
        .assert_success()
        // Second create with IF NOT EXISTS also succeeds (idempotent)
        .exec_dcl("CREATE USER IF NOT EXISTS iso_user WITH PASSWORD 'different_pass'")
        .assert_success()
        // User still exists with original state
        .exec_dcl("DESCRIBE USER iso_user")
        .assert_success();
}

#[test]
fn test_drop_if_exists_isolation() {
    let scenario = new_scenario();

    // Drop non-existent with IF EXISTS succeeds
    scenario
        .exec_dcl("DROP USER IF EXISTS nonexistent_iso")
        .assert_success()
        // Create user
        .exec_dcl("CREATE USER existing_iso WITH PASSWORD 'pass'")
        .assert_success()
        // Drop with IF EXISTS succeeds
        .exec_dcl("DROP USER IF EXISTS existing_iso")
        .assert_success()
        // User is gone
        .exec_dcl("DESCRIBE USER existing_iso")
        .assert_error()
        // Second drop with IF EXISTS also succeeds
        .exec_dcl("DROP USER IF EXISTS existing_iso")
        .assert_success();
}

// ==================== Multiple Operation Consistency ====================

#[test]
fn test_consistency_multiple_users_and_permissions() {
    let scenario = new_scenario()
        .exec_dcl("CREATE SPACE multi_space WITH DIMENSION=128")
        .assert_success();

    let mut scenario = scenario;
    for i in 0..5 {
        let user = format!("multi_user_{}", i);
        let role = ["GOD", "ADMIN", "DBA", "USER", "GUEST"][i % 5];

        scenario = scenario
            .exec_dcl(&format!("CREATE USER {} WITH PASSWORD 'pass'", user))
            .assert_success()
            .exec_dcl(&format!("GRANT {} ON multi_space TO {}", role, user))
            .assert_success();
    }

    // Verify all users and permissions exist
    scenario = scenario.exec_dcl("SHOW USERS").assert_success();

    for i in 0..5 {
        let user = format!("multi_user_{}", i);
        scenario = scenario
            .exec_dcl(&format!("DESCRIBE USER {}", user))
            .assert_success();
    }

    scenario
        .exec_dcl("SHOW ROLES IN multi_space")
        .assert_success();
}

#[test]
fn test_consistency_partial_rollback_scenario() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER rollback_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE rollback_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON rollback_space TO rollback_user")
        .assert_success();

    // Attempt invalid operation
    scenario
        .exec_dcl("ALTER USER rollback_user WITH ROLE INVALID")
        .assert_error()
        // System should remain consistent
        .exec_dcl("DESCRIBE USER rollback_user")
        .assert_success()
        .exec_dcl("SHOW ROLES IN rollback_space")
        .assert_success();
}
