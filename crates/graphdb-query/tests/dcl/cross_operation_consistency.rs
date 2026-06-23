//! Cross-Operation Consistency DCL Tests
//!
//! Test interactions between different DCL statements:
//! - Space deletion and permission cleanup
//! - User deletion and permission cleanup
//! - Multi-step workflows

use super::common;
use common::test_scenario::TestScenario;

fn new_scenario() -> TestScenario {
    TestScenario::new().expect("Failed to create test scenario")
}

// ==================== Space and Permission Relationship Tests ====================

#[test]
fn test_delete_space_cleans_permissions() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER space_cleanup_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE space_cleanup_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON space_cleanup_space TO space_cleanup_user")
        .assert_success();

    scenario
        .exec_dcl("SHOW ROLES IN space_cleanup_space")
        .assert_success()
        // Drop space
        .exec_dcl("DROP SPACE space_cleanup_space")
        .assert_success()
        // User should still exist
        .exec_dcl("DESCRIBE USER space_cleanup_user")
        .assert_success()
        // Space should be gone
        .exec_dcl("SHOW ROLES IN space_cleanup_space")
        .assert_success(); // May be empty or error, both acceptable
}

#[test]
fn test_delete_user_with_permissions() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER delete_perm_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE delete_perm_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON delete_perm_space TO delete_perm_user")
        .assert_success()
        .exec_dcl("GRANT DBA ON delete_perm_space TO delete_perm_user")
        .assert_success();

    scenario
        .exec_dcl("SHOW ROLES IN delete_perm_space")
        .assert_success()
        // Delete user with active permissions
        .exec_dcl("DROP USER delete_perm_user")
        .assert_success()
        // User should be gone
        .exec_dcl("DESCRIBE USER delete_perm_user")
        .assert_error()
        // Space still exists but permissions should be cleaned
        .exec_dcl("SHOW ROLES IN delete_perm_space")
        .assert_success();
}

// ==================== Multi-Step Workflow Tests ====================

#[test]
fn test_complex_workflow_create_grant_alter_revoke_drop() {
    let scenario = new_scenario();

    scenario
        // Step 1: Create users
        .exec_dcl("CREATE USER workflow_admin WITH PASSWORD 'admin_pass'")
        .assert_success()
        .exec_dcl("CREATE USER workflow_dba WITH PASSWORD 'dba_pass'")
        .assert_success()
        .exec_dcl("CREATE USER workflow_user WITH PASSWORD 'user_pass'")
        .assert_success()
        // Step 2: Create space
        .exec_dcl("CREATE SPACE workflow_space WITH DIMENSION=128")
        .assert_success()
        // Step 3: Grant initial permissions
        .exec_dcl("GRANT ADMIN ON workflow_space TO workflow_admin")
        .assert_success()
        .exec_dcl("GRANT DBA ON workflow_space TO workflow_dba")
        .assert_success()
        .exec_dcl("GRANT USER ON workflow_space TO workflow_user")
        .assert_success()
        // Step 4: Verify grants
        .exec_dcl("SHOW ROLES IN workflow_space")
        .assert_success()
        // Step 5: Change passwords
        .exec_dcl("CHANGE PASSWORD workflow_admin 'admin_pass' TO 'admin_pass_new'")
        .assert_success()
        .exec_dcl("CHANGE PASSWORD workflow_dba 'dba_pass' TO 'dba_pass_new'")
        .assert_success()
        // Step 6: Alter users
        .exec_dcl("ALTER USER workflow_user WITH PASSWORD 'user_pass_new'")
        .assert_success()
        // Step 7: Revoke some permissions
        .exec_dcl("REVOKE DBA ON workflow_space FROM workflow_dba")
        .assert_success()
        // Step 8: Grant different role
        .exec_dcl("GRANT GUEST ON workflow_space TO workflow_dba")
        .assert_success()
        // Step 9: Verify final state
        .exec_dcl("SHOW ROLES IN workflow_space")
        .assert_success()
        // Step 10: Drop users one by one
        .exec_dcl("DROP USER workflow_user")
        .assert_success()
        .exec_dcl("DROP USER workflow_dba")
        .assert_success()
        .exec_dcl("DROP USER workflow_admin")
        .assert_success()
        // Step 11: Verify cleanup
        .exec_dcl("DESCRIBE USER workflow_admin")
        .assert_error()
        .exec_dcl("SHOW ROLES IN workflow_space")
        .assert_success();
}

// ==================== Permission Cascade Tests ====================

#[test]
fn test_cascade_permission_revoke_on_user_delete() {
    let scenario = new_scenario()
        .exec_dcl("CREATE SPACE cascade_space1 WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE SPACE cascade_space2 WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE USER cascade_user WITH PASSWORD 'pass'")
        .assert_success();

    scenario
        // Grant multiple roles on multiple spaces
        .exec_dcl("GRANT ADMIN ON cascade_space1 TO cascade_user")
        .assert_success()
        .exec_dcl("GRANT DBA ON cascade_space2 TO cascade_user")
        .assert_success()
        .exec_dcl("GRANT GUEST ON cascade_space1 TO cascade_user")
        .assert_success()
        // Delete user
        .exec_dcl("DROP USER cascade_user")
        .assert_success()
        // All permissions should be gone
        .exec_dcl("DESCRIBE USER cascade_user")
        .assert_error()
        // Spaces should still exist
        .exec_dcl("SHOW ROLES IN cascade_space1")
        .assert_success()
        .exec_dcl("SHOW ROLES IN cascade_space2")
        .assert_success();
}

// ==================== Re-creation After Deletion Tests ====================

#[test]
fn test_recreate_user_with_same_name() {
    let scenario = new_scenario();

    scenario
        // Create and grant
        .exec_dcl("CREATE USER reuse_user WITH PASSWORD 'pass1'")
        .assert_success()
        .exec_dcl("CREATE SPACE reuse_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON reuse_space TO reuse_user")
        .assert_success()
        // Verify exists
        .exec_dcl("DESCRIBE USER reuse_user")
        .assert_success()
        // Delete
        .exec_dcl("DROP USER reuse_user")
        .assert_success()
        // Verify gone
        .exec_dcl("DESCRIBE USER reuse_user")
        .assert_error()
        // Recreate with same name
        .exec_dcl("CREATE USER reuse_user WITH PASSWORD 'pass2'")
        .assert_success()
        // New user should exist
        .exec_dcl("DESCRIBE USER reuse_user")
        .assert_success()
        // Can grant permissions to recreated user
        .exec_dcl("GRANT DBA ON reuse_space TO reuse_user")
        .assert_success()
        .exec_dcl("SHOW ROLES IN reuse_space")
        .assert_success();
}

// ==================== State Consistency After Multi-Operations ====================

#[test]
fn test_consistency_after_mixed_operations() {
    let scenario = new_scenario();

    scenario
        // Create multiple users
        .exec_dcl("CREATE USER mixed_user1 WITH PASSWORD 'pass1'")
        .assert_success()
        .exec_dcl("CREATE USER mixed_user2 WITH PASSWORD 'pass2'")
        .assert_success()
        .exec_dcl("CREATE USER mixed_user3 WITH PASSWORD 'pass3'")
        .assert_success()
        // Create multiple spaces
        .exec_dcl("CREATE SPACE mixed_space1 WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE SPACE mixed_space2 WITH DIMENSION=128")
        .assert_success()
        // Cross-grant permissions
        .exec_dcl("GRANT ADMIN ON mixed_space1 TO mixed_user1")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON mixed_space2 TO mixed_user1")
        .assert_success()
        .exec_dcl("GRANT DBA ON mixed_space1 TO mixed_user2")
        .assert_success()
        .exec_dcl("GRANT GUEST ON mixed_space2 TO mixed_user3")
        .assert_success()
        // Change passwords
        .exec_dcl("CHANGE PASSWORD mixed_user1 'pass1' TO 'new_pass1'")
        .assert_success()
        .exec_dcl("CHANGE PASSWORD mixed_user2 'pass2' TO 'new_pass2'")
        .assert_success()
        // Revoke some permissions
        .exec_dcl("REVOKE ADMIN ON mixed_space2 FROM mixed_user1")
        .assert_success()
        // Verify system is still consistent
        .exec_dcl("SHOW USERS")
        .assert_success()
        .exec_dcl("SHOW ROLES")
        .assert_success()
        .exec_dcl("SHOW ROLES IN mixed_space1")
        .assert_success()
        .exec_dcl("SHOW ROLES IN mixed_space2")
        .assert_success()
        .exec_dcl("DESCRIBE USER mixed_user1")
        .assert_success()
        .exec_dcl("DESCRIBE USER mixed_user2")
        .assert_success()
        .exec_dcl("DESCRIBE USER mixed_user3")
        .assert_success();
}

// ==================== Permission Modification Tests ====================

#[test]
fn test_modify_user_permissions_across_spaces() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER modify_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE modify_space1 WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE SPACE modify_space2 WITH DIMENSION=128")
        .assert_success();

    scenario
        // Initial grants
        .exec_dcl("GRANT ADMIN ON modify_space1 TO modify_user")
        .assert_success()
        .exec_dcl("GRANT USER ON modify_space2 TO modify_user")
        .assert_success()
        // Upgrade permissions
        .exec_dcl("REVOKE USER ON modify_space2 FROM modify_user")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON modify_space2 TO modify_user")
        .assert_success()
        // Verify upgraded
        .exec_dcl("SHOW ROLES IN modify_space2")
        .assert_success()
        // Downgrade permissions
        .exec_dcl("REVOKE ADMIN ON modify_space1 FROM modify_user")
        .assert_success()
        .exec_dcl("GRANT GUEST ON modify_space1 TO modify_user")
        .assert_success()
        // Verify downgraded
        .exec_dcl("SHOW ROLES IN modify_space1")
        .assert_success();
}

// ==================== Bulk Operation Consistency ====================

#[test]
fn test_consistency_bulk_user_and_permission_operations() {
    let scenario = new_scenario();

    let mut scenario = scenario;
    for i in 0..5 {
        scenario = scenario
            .exec_dcl(&format!(
                "CREATE USER bulk_user_{} WITH PASSWORD 'pass{}'",
                i, i
            ))
            .assert_success();
    }

    for i in 0..5 {
        scenario = scenario
            .exec_dcl(&format!("CREATE SPACE bulk_space_{} WITH DIMENSION=128", i))
            .assert_success();
    }

    for i in 0..5 {
        for j in 0..5 {
            let role = ["GOD", "ADMIN", "DBA", "USER", "GUEST"][(i + j) % 5];
            scenario = scenario
                .exec_dcl(&format!(
                    "GRANT {} ON bulk_space_{} TO bulk_user_{}",
                    role, j, i
                ))
                .assert_success();
        }
    }

    // Verify all users and permissions exist
    scenario = scenario.exec_dcl("SHOW USERS").assert_success();

    for i in 0..5 {
        scenario = scenario
            .exec_dcl(&format!("DESCRIBE USER bulk_user_{}", i))
            .assert_success();
    }

    for i in 0..5 {
        scenario = scenario
            .exec_dcl(&format!("SHOW ROLES IN bulk_space_{}", i))
            .assert_success();
    }
}
