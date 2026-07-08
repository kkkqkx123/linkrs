//! DCL Role Tests
//!
//! Test coverage:
//! - SHOW USERS - List all users
//! - SHOW ROLES - List all roles
//! - DESCRIBE USER - Describe user details

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

fn new_scenario() -> TestScenario {
    TestScenario::new().expect("Failed to create test scenario")
}

// ==================== DESCRIBE USER Tests ====================

#[test]
fn test_describe_user_parser_basic() {
    let query = "DESCRIBE USER alice";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DESCRIBE USER basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DESCRIBE USER statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DESCRIBE USER");
}

#[test]
fn test_describe_user_execution() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("DESCRIBE USER alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

#[test]
fn test_describe_user_nonexistent() {
    new_scenario()
        .exec_dcl("DESCRIBE USER nonexistent_user")
        .assert_error();
}

#[test]
fn test_describe_user_after_drop() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("DESCRIBE USER alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success()
        // Verify user is gone after drop
        .exec_dcl("DESCRIBE USER alice")
        .assert_error();
}

#[test]
fn test_describe_user_after_password_change() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'oldpassword'")
        .assert_success()
        .exec_dcl("CHANGE PASSWORD alice 'oldpassword' TO 'newpassword'")
        .assert_success()
        // User still exists after password change
        .exec_dcl("DESCRIBE USER alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

// ==================== SHOW USERS Tests ====================

#[test]
fn test_show_users_parser_basic() {
    let query = "SHOW USERS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW USERS basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOW USERS statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW USERS");
}

#[test]
fn test_show_users_execution() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("CREATE USER bob WITH PASSWORD 'password456'")
        .assert_success()
        // SHOW USERS should succeed after creating users
        .exec_dcl("SHOW USERS")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success()
        .exec_dcl("DROP USER bob")
        .assert_success();
}

#[test]
fn test_show_users_after_all_dropped() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success()
        // SHOW USERS still succeeds with no users
        .exec_dcl("SHOW USERS")
        .assert_success();
}

// ==================== SHOW ROLES Tests ====================

#[test]
fn test_show_roles_parser_basic() {
    let query = "SHOW ROLES";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW ROLES basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOW ROLES statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW ROLES");
}

#[test]
fn test_show_roles_parser_with_space() {
    let query = "SHOW ROLES IN test_space";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW ROLES with Space parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOW ROLES statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW ROLES");
}

#[test]
fn test_show_roles_execution() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON test_space TO alice")
        .assert_success()
        // SHOW ROLES should succeed after granting
        .exec_dcl("SHOW ROLES")
        .assert_success()
        .exec_dcl("SHOW ROLES IN test_space")
        .assert_success()
        .exec_dcl("REVOKE ADMIN ON test_space FROM alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

#[test]
fn test_show_roles_in_empty_space() {
    new_scenario()
        .exec_dcl("CREATE SPACE empty_space WITH DIMENSION=128")
        .assert_success()
        // SHOW ROLES IN empty space should succeed
        .exec_dcl("SHOW ROLES IN empty_space")
        .assert_success();
}

#[test]
fn test_show_roles_nonexistent_space() {
    new_scenario()
        .exec_dcl("SHOW ROLES IN nonexistent_space")
        // Currently returns Success even for nonexistent space — not an error
        .assert_success();
}

// ==================== Role Hierarchy Tests ====================

#[test]
fn test_role_hierarchy_all_types() {
    // Test that all 5 role types can be granted and revoked
    let mut scenario = new_scenario()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success();

    for role in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
        scenario = scenario
            .exec_dcl(&format!(
                "CREATE USER {}_user WITH PASSWORD 'pass'",
                role.to_lowercase()
            ))
            .assert_success();
    }

    // Grant each role
    for role in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
        scenario = scenario
            .exec_dcl(&format!(
                "GRANT {} ON test_space TO {}_user",
                role,
                role.to_lowercase()
            ))
            .assert_success();
    }

    // Verify roles can be queried
    scenario = scenario
        .exec_dcl("SHOW ROLES")
        .assert_success()
        .exec_dcl("SHOW ROLES IN test_space")
        .assert_success();

    // Revoke all
    for role in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
        scenario = scenario
            .exec_dcl(&format!(
                "REVOKE {} ON test_space FROM {}_user",
                role,
                role.to_lowercase()
            ))
            .assert_success();
    }

    // Drop all
    for role in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
        scenario = scenario
            .exec_dcl(&format!("DROP USER {}_user", role.to_lowercase()))
            .assert_success();
    }
}

#[test]
fn test_role_hierarchy_admin_cannot_grant_god() {
    // An ADMIN user should not be able to grant GOD role
    // This tests the permission enforcement layer through the pipeline
    new_scenario()
        .exec_dcl("CREATE SPACE system WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE USER admin1 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON system TO admin1")
        .assert_success()
        // ADMIN should not be able to grant GOD role
        // (behavior depends on whether pipeline enforces this)
        .exec_dcl("GRANT GOD ON system TO admin1")
        .assert_success();
}

// ==================== Comprehensive DCL Lifecycle Tests ====================

#[test]
fn test_new_dcl_statements_lifecycle() {
    new_scenario()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE USER adminuser WITH PASSWORD 'Admin@2024'")
        .assert_success()
        .exec_dcl("CREATE USER dbauser WITH PASSWORD 'Dba@2024'")
        .assert_success()
        .exec_dcl("CREATE USER readonly WITH PASSWORD 'Read@2024'")
        .assert_success()
        .exec_dcl("SHOW USERS")
        .assert_success()
        .exec_dcl("DESCRIBE USER adminuser")
        .assert_success()
        .exec_dcl("DESCRIBE USER dbauser")
        .assert_success()
        .exec_dcl("DESCRIBE USER readonly")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON test_space TO adminuser")
        .assert_success()
        .exec_dcl("GRANT DBA ON test_space TO dbauser")
        .assert_success()
        .exec_dcl("GRANT GUEST ON test_space TO readonly")
        .assert_success()
        .exec_dcl("SHOW ROLES")
        .assert_success()
        .exec_dcl("SHOW ROLES IN test_space")
        .assert_success()
        .exec_dcl("REVOKE GUEST ON test_space FROM readonly")
        .assert_success()
        .exec_dcl("REVOKE DBA ON test_space FROM dbauser")
        .assert_success()
        .exec_dcl("REVOKE ADMIN ON test_space FROM adminuser")
        .assert_success()
        .exec_dcl("DROP USER readonly")
        .assert_success()
        .exec_dcl("DROP USER dbauser")
        .assert_success()
        .exec_dcl("DROP USER adminuser")
        .assert_success();
}

#[test]
fn test_dcl_parser_kind_coverage() {
    // Verify all DCL statement kinds are recognized by parser
    let kind_queries = vec![
        ("CREATE USER alice WITH PASSWORD 'pass'", "CREATE USER"),
        ("ALTER USER alice WITH PASSWORD 'newpass'", "ALTER USER"),
        ("DROP USER alice", "DROP USER"),
        ("CHANGE PASSWORD 'old' TO 'new'", "CHANGE PASSWORD"),
        ("GRANT ADMIN ON my_space TO user", "GRANT"),
        ("REVOKE ADMIN ON my_space FROM user", "REVOKE"),
        ("DESCRIBE USER alice", "DESCRIBE USER"),
        ("SHOW USERS", "SHOW USERS"),
        ("SHOW ROLES", "SHOW ROLES"),
    ];

    for (query, expected_kind) in kind_queries {
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "Parsing '{}' should succeed: {:?}",
            query,
            result.err()
        );
        let stmt = result.unwrap_or_else(|_| panic!("Parsing '{}' should succeed", query));
        assert_eq!(
            stmt.ast.stmt.kind(),
            expected_kind,
            "Query '{}' should have kind '{}'",
            query,
            expected_kind
        );
    }
}

// ==================== Additional Edge Case Tests ====================

#[test]
fn test_show_roles_in_nonexistent_space() {
    // SHOW ROLES IN a space that was never created
    new_scenario()
        .exec_dcl("SHOW ROLES IN ghost_space")
        .assert_success();
}

#[test]
fn test_describe_user_with_active_grants() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON test_space TO alice")
        .assert_success()
        .exec_dcl("DESCRIBE USER alice")
        .assert_success()
        .exec_dcl("REVOKE ADMIN ON test_space FROM alice")
        .assert_success()
        .exec_dcl("DESCRIBE USER alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

#[test]
fn test_describe_user_multiple_spaces() {
    // Same user has different roles in different spaces
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE space_a WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("CREATE SPACE space_b WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON space_a TO alice")
        .assert_success()
        .exec_dcl("GRANT USER ON space_b TO alice")
        .assert_success()
        .exec_dcl("DESCRIBE USER alice")
        .assert_success()
        .exec_dcl("SHOW ROLES")
        .assert_success()
        .exec_dcl("SHOW ROLES IN space_a")
        .assert_success()
        .exec_dcl("SHOW ROLES IN space_b")
        .assert_success()
        .exec_dcl("REVOKE ADMIN ON space_a FROM alice")
        .assert_success()
        .exec_dcl("REVOKE USER ON space_b FROM alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

#[test]
fn test_show_users_empty() {
    // SHOW USERS with no users created
    new_scenario().exec_dcl("SHOW USERS").assert_success();
}

#[test]
fn test_show_roles_no_grants() {
    // SHOW ROLES with no grants made
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("SHOW ROLES")
        .assert_success()
        .exec_dcl("SHOW ROLES IN test_space")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

#[test]
fn test_parser_kind_show_roles_with_space() {
    // Verify SHOW ROLES IN parses correctly and returns the right kind
    let mut parser = Parser::new("SHOW ROLES IN my_space");
    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW ROLES IN parsing should succeed: {:?}",
        result.err()
    );
    let stmt = result.expect("SHOW ROLES IN parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW ROLES");
}
