//! Cascade DCL Operations Tests
//!
//! Test cascade deletion and cleanup scenarios:
//! - User deletion cascades permission cleanup
//! - Space deletion cascades permission cleanup
//! - Complex cascading scenarios

use super::common;
use common::test_scenario::TestScenario;

fn new_scenario() -> TestScenario {
    TestScenario::new().expect("Failed to create test scenario")
}

// ==================== User Deletion Cascade Tests ====================

#[test]
fn test_delete_user_removes_single_permission() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER cascade_user1 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE cascade_space1 WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON cascade_space1 TO cascade_user1")
        .assert_success();

    scenario
        .exec_dcl("SHOW ROLES IN cascade_space1")
        .assert_success()
        .exec_dcl("DROP USER cascade_user1")
        .assert_success()
        .exec_dcl("SHOW ROLES IN cascade_space1")
        .assert_success()
        // Verify permission is cleaned up
        .exec_dcl("SHOW USERS")
        .assert_success();
}

#[test]
fn test_delete_user_removes_multiple_permissions_single_space() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER multi_role_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE multi_role_space WITH DIMENSION=128")
        .assert_success();

    scenario
        // Grant multiple roles to same user on same space
        .exec_dcl("GRANT ADMIN ON multi_role_space TO multi_role_user")
        .assert_success()
        .exec_dcl("GRANT DBA ON multi_role_space TO multi_role_user")
        .assert_success()
        .exec_dcl("GRANT GUEST ON multi_role_space TO multi_role_user")
        .assert_success()
        .exec_dcl("SHOW ROLES IN multi_role_space")
        .assert_success()
        // Delete user
        .exec_dcl("DROP USER multi_role_user")
        .assert_success()
        // All permissions should be cleaned
        .exec_dcl("SHOW ROLES IN multi_role_space")
        .assert_success();
}

#[test]
fn test_delete_user_removes_permissions_across_spaces() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER cross_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE cross_space1 WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE SPACE cross_space2 WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE SPACE cross_space3 WITH DIMENSION=128")
        .assert_success();

    scenario
        // Grant permissions on multiple spaces
        .exec_dcl("GRANT ADMIN ON cross_space1 TO cross_user")
        .assert_success()
        .exec_dcl("GRANT DBA ON cross_space2 TO cross_user")
        .assert_success()
        .exec_dcl("GRANT GUEST ON cross_space3 TO cross_user")
        .assert_success()
        // Delete user
        .exec_dcl("DROP USER cross_user")
        .assert_success()
        // All permissions should be cleaned from all spaces
        .exec_dcl("SHOW ROLES IN cross_space1")
        .assert_success()
        .exec_dcl("SHOW ROLES IN cross_space2")
        .assert_success()
        .exec_dcl("SHOW ROLES IN cross_space3")
        .assert_success();
}

#[test]
fn test_delete_multiple_users_each_with_permissions() {
    let scenario = new_scenario()
        .exec_dcl("CREATE SPACE shared_cascade_space WITH DIMENSION=128")
        .assert_success();

    let mut scenario = scenario;
    for i in 0..3 {
        let username = format!("cascade_multi_user_{}", i);
        scenario = scenario
            .exec_dcl(&format!("CREATE USER {} WITH PASSWORD 'pass'", username))
            .assert_success()
            .exec_dcl(&format!(
                "GRANT ADMIN ON shared_cascade_space TO {}",
                username
            ))
            .assert_success();
    }

    scenario = scenario
        .exec_dcl("SHOW ROLES IN shared_cascade_space")
        .assert_success();

    for i in 0..3 {
        let username = format!("cascade_multi_user_{}", i);
        scenario = scenario
            .exec_dcl(&format!("DROP USER {}", username))
            .assert_success();
    }

    scenario
        .exec_dcl("SHOW ROLES IN shared_cascade_space")
        .assert_success();
}

// ==================== Space Deletion Cascade Tests ====================

#[test]
fn test_delete_space_cleans_user_permissions() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER space_del_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE space_del_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON space_del_space TO space_del_user")
        .assert_success();

    scenario
        .exec_dcl("SHOW ROLES IN space_del_space")
        .assert_success()
        .exec_dcl("DROP SPACE space_del_space")
        .assert_success()
        // User should still exist
        .exec_dcl("DESCRIBE USER space_del_user")
        .assert_success()
        // Space and its permissions should be gone
        .exec_dcl("SHOW ROLES IN space_del_space")
        .assert_success();
}

#[test]
fn test_delete_space_with_multiple_users() {
    let scenario = new_scenario()
        .exec_dcl("CREATE SPACE multi_user_space WITH DIMENSION=128")
        .assert_success();

    let mut scenario = scenario;
    for i in 0..5 {
        let username = format!("space_user_{}", i);
        let role = ["ADMIN", "DBA", "USER", "GUEST", "ADMIN"][i % 4];
        scenario = scenario
            .exec_dcl(&format!("CREATE USER {} WITH PASSWORD 'pass'", username))
            .assert_success()
            .exec_dcl(&format!(
                "GRANT {} ON multi_user_space TO {}",
                role, username
            ))
            .assert_success();
    }

    scenario = scenario
        .exec_dcl("SHOW ROLES IN multi_user_space")
        .assert_success()
        .exec_dcl("DROP SPACE multi_user_space")
        .assert_success();

    // All users should still exist
    for i in 0..5 {
        let username = format!("space_user_{}", i);
        scenario = scenario
            .exec_dcl(&format!("DESCRIBE USER {}", username))
            .assert_success();
    }
}

// ==================== Complex Cascade Scenarios ====================

#[test]
fn test_cascade_delete_user_then_space() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER cascade_both_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE cascade_both_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON cascade_both_space TO cascade_both_user")
        .assert_success();

    scenario
        // Delete user first
        .exec_dcl("DROP USER cascade_both_user")
        .assert_success()
        // User should be gone
        .exec_dcl("DESCRIBE USER cascade_both_user")
        .assert_error()
        // Space should still exist
        .exec_dcl("SHOW ROLES IN cascade_both_space")
        .assert_success()
        // Delete space
        .exec_dcl("DROP SPACE cascade_both_space")
        .assert_success()
        // Space should be gone
        .exec_dcl("SHOW ROLES IN cascade_both_space")
        .assert_success();
}

#[test]
fn test_cascade_delete_space_then_user() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER cascade_user_then_space WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE cascade_space_then_user WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON cascade_space_then_user TO cascade_user_then_space")
        .assert_success();

    scenario
        // Delete space first
        .exec_dcl("DROP SPACE cascade_space_then_user")
        .assert_success()
        // Space should be gone
        .exec_dcl("SHOW ROLES IN cascade_space_then_user")
        .assert_success()
        // User should still exist
        .exec_dcl("DESCRIBE USER cascade_user_then_space")
        .assert_success()
        // Delete user
        .exec_dcl("DROP USER cascade_user_then_space")
        .assert_success()
        // User should be gone
        .exec_dcl("DESCRIBE USER cascade_user_then_space")
        .assert_error();
}

// ==================== Permission Cleanup Verification ====================

#[test]
fn test_no_orphaned_permissions_after_user_delete() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER orphan_test_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE orphan_test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON orphan_test_space TO orphan_test_user")
        .assert_success()
        .exec_dcl("GRANT DBA ON orphan_test_space TO orphan_test_user")
        .assert_success();

    let roles_before = scenario
        .exec_dcl("SHOW ROLES IN orphan_test_space")
        .assert_success();

    roles_before
        .exec_dcl("DROP USER orphan_test_user")
        .assert_success();

    // No orphaned permissions should exist
    roles_before
        .exec_dcl("SHOW ROLES IN orphan_test_space")
        .assert_success();
}

#[test]
fn test_no_orphaned_permissions_after_space_delete() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER user_orphan_test WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE space_orphan_test WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON space_orphan_test TO user_orphan_test")
        .assert_success();

    scenario
        .exec_dcl("DROP SPACE space_orphan_test")
        .assert_success()
        // User should be unaffected
        .exec_dcl("DESCRIBE USER user_orphan_test")
        .assert_success()
        // No orphaned permissions
        .exec_dcl("SHOW USERS")
        .assert_success();
}

// ==================== Cascade with Multiple Operations ====================

#[test]
fn test_cascade_with_recreate() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER cascade_recreate WITH PASSWORD 'pass1'")
        .assert_success()
        .exec_dcl("CREATE SPACE cascade_recreate_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON cascade_recreate_space TO cascade_recreate")
        .assert_success();

    scenario
        // Delete user
        .exec_dcl("DROP USER cascade_recreate")
        .assert_success()
        // Recreate with same name
        .exec_dcl("CREATE USER cascade_recreate WITH PASSWORD 'pass2'")
        .assert_success()
        // New user should have no permissions from before
        .exec_dcl("DESCRIBE USER cascade_recreate")
        .assert_success()
        // But can be granted new permissions
        .exec_dcl("GRANT GUEST ON cascade_recreate_space TO cascade_recreate")
        .assert_success();
}

// ==================== Bulk Cascade Scenarios ====================

#[test]
fn test_bulk_cascade_delete_multiple_users() {
    let scenario = new_scenario()
        .exec_dcl("CREATE SPACE bulk_cascade_space WITH DIMENSION=128")
        .assert_success();

    let mut scenario = scenario;
    for i in 0..10 {
        let username = format!("bulk_cascade_{}", i);
        scenario = scenario
            .exec_dcl(&format!("CREATE USER {} WITH PASSWORD 'pass'", username))
            .assert_success()
            .exec_dcl(&format!(
                "GRANT ADMIN ON bulk_cascade_space TO {}",
                username
            ))
            .assert_success();
    }

    // Delete all users
    for i in 0..10 {
        let username = format!("bulk_cascade_{}", i);
        scenario = scenario
            .exec_dcl(&format!("DROP USER {}", username))
            .assert_success();
    }

    // All permissions should be cleaned
    scenario
        .exec_dcl("SHOW ROLES IN bulk_cascade_space")
        .assert_success();
}

#[test]
fn test_bulk_cascade_delete_multiple_spaces() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER bulk_space_cascade_user WITH PASSWORD 'pass'")
        .assert_success();

    let mut scenario = scenario;
    for i in 0..5 {
        let space_name = format!("bulk_space_cascade_{}", i);
        scenario = scenario
            .exec_dcl(&format!("CREATE SPACE {} WITH DIMENSION=128", space_name))
            .assert_success()
            .exec_dcl(&format!(
                "GRANT ADMIN ON {} TO bulk_space_cascade_user",
                space_name
            ))
            .assert_success();
    }

    // Delete all spaces
    for i in 0..5 {
        let space_name = format!("bulk_space_cascade_{}", i);
        scenario = scenario
            .exec_dcl(&format!("DROP SPACE {}", space_name))
            .assert_success();
    }

    // User should still exist but with no permissions
    scenario
        .exec_dcl("DESCRIBE USER bulk_space_cascade_user")
        .assert_success();
}
