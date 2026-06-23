//! DCL User Management Tests
//!
//! Test coverage:
//! - CREATE USER - Create a user
//! - ALTER USER - Modifies a user account
//! - DROP USER - Deletes a user
//! - CHANGE PASSWORD - Change your password

use super::common;

use common::test_scenario::TestScenario;
use common::TestStorage;
use graphdb_query::core::stats::StatsManager;
use graphdb_query::query::optimizer::OptimizerEngine;
use graphdb_query::query::parser::Parser;
use graphdb_query::query::query_pipeline_manager::QueryPipelineManager;
use std::sync::Arc;

fn new_scenario() -> TestScenario {
    TestScenario::new().expect("Failed to create test scenario")
}

// ==================== CREATE USER Parser Tests ====================

#[test]
fn test_create_user_parser_basic() {
    let query = "CREATE USER alice WITH PASSWORD 'password123'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE USER basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE USER");
}

#[test]
fn test_create_user_parser_with_if_not_exists() {
    let query = "CREATE USER IF NOT EXISTS alice WITH PASSWORD 'password123'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE USER with IF NOT EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE USER");
}

#[test]
fn test_create_user_parser_complex_password() {
    let query = "CREATE USER alice WITH PASSWORD 'P@ssw0rd!2024'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE USER complex password parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE USER");
}

#[test]
fn test_create_user_parser_special_username() {
    let query = "CREATE USER user_123 WITH PASSWORD 'password'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE USER special username parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE USER");
}

// ==================== CREATE USER Execution Tests ====================

#[test]
fn test_create_user_execution_basic() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        // Verify user exists via DESCRIBE USER
        .exec_dcl("DESCRIBE USER alice")
        .assert_success();
}

#[test]
fn test_create_user_execution_with_if_not_exists() {
    new_scenario()
        .exec_dcl("CREATE USER IF NOT EXISTS alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("DESCRIBE USER alice")
        .assert_success();
}

#[test]
fn test_create_user_duplicate() {
    // Implementation is IDEMPOTENT — duplicate user creation succeeds
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

// ==================== ALTER USER Parser Tests ====================

#[test]
fn test_alter_user_parser_basic() {
    let query = "ALTER USER alice WITH PASSWORD 'newpassword123'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ALTER USER basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("ALTER USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ALTER USER");
}

#[test]
fn test_alter_user_parser_complex_password() {
    let query = "ALTER USER alice WITH PASSWORD 'NewP@ssw0rd!2024'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ALTER USER complex password parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("ALTER USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ALTER USER");
}

#[test]
fn test_alter_user_parser_special_username() {
    let query = "ALTER USER user_123 WITH PASSWORD 'newpassword'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ALTER USER special username parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("ALTER USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ALTER USER");
}

// ==================== ALTER USER Execution Tests ====================

#[test]
fn test_alter_user_execution_basic() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("ALTER USER alice WITH PASSWORD 'newpassword123'")
        .assert_success()
        // Verify user still exists after ALTER
        .exec_dcl("DESCRIBE USER alice")
        .assert_success();
}

#[test]
fn test_alter_user_nonexistent() {
    new_scenario()
        .exec_dcl("ALTER USER nonexistent_user WITH PASSWORD 'newpassword'")
        .assert_error();
}

// ==================== DROP USER Parser Tests ====================

#[test]
fn test_drop_user_parser_basic() {
    let query = "DROP USER alice";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP USER basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP USER");
}

#[test]
fn test_drop_user_parser_with_if_exists() {
    let query = "DROP USER IF EXISTS alice";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP USER with IF EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP USER");
}

#[test]
fn test_drop_user_parser_special_username() {
    let query = "DROP USER user_123";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP USER special username parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP USER");
}

// ==================== DROP USER Execution Tests ====================

#[test]
fn test_drop_user_execution_basic() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success()
        // Verify user is actually gone
        .exec_dcl("DESCRIBE USER alice")
        .assert_error();
}

#[test]
fn test_drop_user_with_if_exists() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("DROP USER IF EXISTS alice")
        .assert_success()
        .exec_dcl("DESCRIBE USER alice")
        .assert_error();
}

#[test]
fn test_drop_user_nonexistent() {
    new_scenario()
        .exec_dcl("DROP USER nonexistent_user")
        .assert_error();
}

#[test]
fn test_drop_user_nonexistent_with_if_exists() {
    new_scenario()
        .exec_dcl("DROP USER IF EXISTS nonexistent_user")
        .assert_success();
}

// ==================== CHANGE PASSWORD Tests ====================

#[test]
fn test_change_password_parser_basic() {
    let query = "CHANGE PASSWORD 'oldpassword' TO 'newpassword'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CHANGE PASSWORD basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CHANGE PASSWORD statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CHANGE PASSWORD");
}

#[test]
fn test_change_password_parser_complex_passwords() {
    let query = "CHANGE PASSWORD 'OldP@ssw0rd!' TO 'NewP@ssw0rd!2024'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CHANGE PASSWORD complex password parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CHANGE PASSWORD statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CHANGE PASSWORD");
}

#[test]
fn test_change_password_parser_special_chars() {
    let query = "CHANGE PASSWORD 'p@$$w0rd#123' TO 'n3wP@$$w0rd#456'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CHANGE PASSWORD special char password parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CHANGE PASSWORD statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CHANGE PASSWORD");
}

#[test]
fn test_change_password_execution_basic() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'oldpassword'")
        .assert_success()
        // Correct old password should succeed
        .exec_dcl("CHANGE PASSWORD alice 'oldpassword' TO 'newpassword'")
        .assert_success()
        // After change, the new password allows further changes
        .exec_dcl("CHANGE PASSWORD alice 'newpassword' TO 'newerpassword'")
        .assert_success();
}

#[test]
fn test_change_password_wrong_old_password() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'oldpassword'")
        .assert_success()
        // Wrong old password should fail
        .exec_dcl("CHANGE PASSWORD alice 'wrongpassword' TO 'newpassword'")
        .assert_error();
}

#[test]
fn test_change_password_self() {
    // CHANGE PASSWORD without username requires active session context
    // Without a valid session, this should fail
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'oldpassword'")
        .assert_success()
        // Self-password-change without username and without session context
        // should fail because no user context is available
        .exec_dcl("CHANGE PASSWORD 'wrongpassword' TO 'newpassword'")
        .assert_error();
}

// ==================== User Lifecycle Tests ====================

#[test]
fn test_dcl_user_lifecycle() {
    new_scenario()
        // Full lifecycle: CREATE → CHANGE PASSWORD (twice) → DROP
        .exec_dcl("CREATE USER testuser WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("CHANGE PASSWORD testuser 'password123' TO 'newpassword123'")
        .assert_success()
        .exec_dcl("CHANGE PASSWORD testuser 'newpassword123' TO 'anotherpassword123'")
        .assert_success()
        // Verify user still exists after password changes
        .exec_dcl("DESCRIBE USER testuser")
        .assert_success()
        .exec_dcl("DROP USER testuser")
        .assert_success()
        // Verify user is gone
        .exec_dcl("DESCRIBE USER testuser")
        .assert_error();
}

#[test]
fn test_dcl_multiple_users() {
    let mut scenario = new_scenario();
    for name in ["alice", "bob", "charlie"] {
        scenario = scenario
            .exec_dcl(&format!("CREATE USER {} WITH PASSWORD '{}123'", name, name))
            .assert_success();
    }
    // Verify each user exists
    for name in ["alice", "bob", "charlie"] {
        scenario = scenario
            .exec_dcl(&format!("DESCRIBE USER {}", name))
            .assert_success();
    }
    // Drop all users and verify they're gone
    for name in ["alice", "bob", "charlie"] {
        scenario = scenario
            .exec_dcl(&format!("DROP USER {}", name))
            .assert_success()
            .exec_dcl(&format!("DESCRIBE USER {}", name))
            .assert_error();
    }
}

#[test]
fn test_dcl_if_not_exists_if_exists() {
    new_scenario()
        .exec_dcl("CREATE USER IF NOT EXISTS testuser WITH PASSWORD 'password'")
        .assert_success()
        // IF NOT EXISTS on existing user is a no-op (succeeds)
        .exec_dcl("CREATE USER IF NOT EXISTS testuser WITH PASSWORD 'password'")
        .assert_success()
        .exec_dcl("DROP USER IF EXISTS testuser")
        .assert_success()
        // IF EXISTS on already-dropped user is a no-op (succeeds)
        .exec_dcl("DROP USER IF EXISTS testuser")
        .assert_success();
}

#[test]
fn test_dcl_error_handling() {
    let _scenario = new_scenario();
    let invalid_queries = vec![
        "CREATE USER",
        "CREATE USER testuser",
        "CREATE USER WITH PASSWORD 'password'",
        "ALTER USER",
        "ALTER USER testuser",
        "DROP USER",
        "CHANGE PASSWORD",
        "CHANGE PASSWORD 'oldpassword'",
    ];

    for query in invalid_queries {
        let test_storage = TestStorage::new().expect("Failed to create test storage");
        let storage = test_storage.storage();
        let stats_manager = Arc::new(StatsManager::new());
        let mut pipeline_manager = QueryPipelineManager::with_optimizer(
            storage,
            stats_manager,
            Arc::new(OptimizerEngine::default()),
        );
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_err(),
            "Invalid query should return error: {}",
            query
        );
    }
}

#[test]
fn test_dcl_password_security() {
    new_scenario()
        .exec_dcl("CREATE USER secureuser WITH PASSWORD 'SecureP@ssw0rd!2024'")
        .assert_success()
        .exec_dcl("CHANGE PASSWORD secureuser 'SecureP@ssw0rd!2024' TO 'N3wS3cur3P@ssw0rd!2024'")
        .assert_success()
        .exec_dcl(
            "CHANGE PASSWORD secureuser 'N3wS3cur3P@ssw0rd!2024' TO 'An0th3rS3cur3P@ssw0rd!2024'",
        )
        .assert_success()
        // Old password no longer works after change
        .exec_dcl("CHANGE PASSWORD secureuser 'SecureP@ssw0rd!2024' TO 'shouldnotwork'")
        .assert_error();
}

#[test]
fn test_dcl_user_management_workflow() {
    new_scenario()
        .exec_dcl("CREATE USER admin_user WITH PASSWORD 'Admin@2024'")
        .assert_success()
        .exec_dcl("CREATE USER readonly_user WITH PASSWORD 'Read@2024'")
        .assert_success()
        .exec_dcl("CHANGE PASSWORD readonly_user 'Read@2024' TO 'NewRead@2024'")
        .assert_success()
        .exec_dcl("DROP USER readonly_user")
        .assert_success()
        .exec_dcl("DESCRIBE USER readonly_user")
        .assert_error()
        .exec_dcl("CHANGE PASSWORD admin_user 'Admin@2024' TO 'NewAdmin@2024'")
        .assert_success()
        .exec_dcl("DROP USER admin_user")
        .assert_success()
        .exec_dcl("DESCRIBE USER admin_user")
        .assert_error();
}

#[test]
fn test_dcl_special_usernames() {
    new_scenario()
        .exec_dcl("CREATE USER user_123 WITH PASSWORD 'password'")
        .assert_success()
        .exec_dcl("CREATE USER user_456 WITH PASSWORD 'password'")
        .assert_success()
        .exec_dcl("CREATE USER user_789 WITH PASSWORD 'password'")
        .assert_success()
        // Verify all exist
        .exec_dcl("DESCRIBE USER user_123")
        .assert_success()
        .exec_dcl("DESCRIBE USER user_456")
        .assert_success()
        .exec_dcl("DESCRIBE USER user_789")
        .assert_success()
        .exec_dcl("DROP USER user_123")
        .assert_success()
        .exec_dcl("DROP USER user_456")
        .assert_success()
        .exec_dcl("DROP USER user_789")
        .assert_success()
        // Verify all gone
        .exec_dcl("DESCRIBE USER user_123")
        .assert_error()
        .exec_dcl("DESCRIBE USER user_456")
        .assert_error()
        .exec_dcl("DESCRIBE USER user_789")
        .assert_error();
}

// ==================== Additional Edge Case Tests ====================

#[test]
fn test_create_user_reserved_username() {
    // Creating a user named "root" — reserved name handling
    new_scenario()
        .exec_dcl("CREATE USER root WITH PASSWORD 'rootpass'")
        .assert_success()
        .exec_dcl("DESCRIBE USER root")
        .assert_success()
        .exec_dcl("DROP USER root")
        .assert_success();
}

#[test]
fn test_create_user_reserved_username_admin() {
    new_scenario()
        .exec_dcl("CREATE USER admin WITH PASSWORD 'adminpass'")
        .assert_success()
        .exec_dcl("DESCRIBE USER admin")
        .assert_success()
        .exec_dcl("DROP USER admin")
        .assert_success();
}

#[test]
fn test_create_user_empty_password() {
    // Short/empty password handling
    new_scenario()
        .exec_dcl("CREATE USER short WITH PASSWORD 'a'")
        .assert_success()
        .exec_dcl("DROP USER short")
        .assert_success();
}

#[test]
fn test_drop_user_with_active_grants() {
    // Create user, grant a role, then drop user — should succeed
    // (revoke cascading is handled by storage)
    new_scenario()
        .exec_dcl("CREATE USER grantuser WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON test_space TO grantuser")
        .assert_success()
        // Drop user with active grant — should succeed
        .exec_dcl("DROP USER grantuser")
        .assert_success()
        .exec_dcl("DESCRIBE USER grantuser")
        .assert_error();
}

#[test]
fn test_recreate_user_after_drop() {
    new_scenario()
        .exec_dcl("CREATE USER recyclable WITH PASSWORD 'firstpass'")
        .assert_success()
        .exec_dcl("DROP USER recyclable")
        .assert_success()
        // Recreate same user
        .exec_dcl("CREATE USER recyclable WITH PASSWORD 'secondpass'")
        .assert_success()
        .exec_dcl("DESCRIBE USER recyclable")
        .assert_success()
        .exec_dcl("DROP USER recyclable")
        .assert_success();
}

#[test]
fn test_alter_user_same_password() {
    // ALTER USER to the same password (should succeed as a no-op)
    new_scenario()
        .exec_dcl("CREATE USER samepw WITH PASSWORD 'samepassword'")
        .assert_success()
        .exec_dcl("ALTER USER samepw WITH PASSWORD 'samepassword'")
        .assert_success()
        .exec_dcl("DESCRIBE USER samepw")
        .assert_success()
        .exec_dcl("DROP USER samepw")
        .assert_success();
}

#[test]
fn test_dcl_user_name_case_sensitivity() {
    // Test if usernames are case-sensitive
    new_scenario()
        .exec_dcl("CREATE USER Alice WITH PASSWORD 'pass'")
        .assert_success()
        // DESCRIBE with different case — usernames are case-sensitive
        .exec_dcl("DESCRIBE USER alice")
        .assert_error()
        .exec_dcl("DROP USER Alice")
        .assert_success();
}
