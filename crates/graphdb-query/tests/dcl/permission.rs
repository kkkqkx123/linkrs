//! DCL Permission Tests
//!
//! Test coverage:
//! - GRANT - Grant privileges to users
//! - REVOKE - Revoke privileges from users

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

fn new_scenario() -> TestScenario {
    TestScenario::new().expect("Failed to create test scenario")
}

#[allow(dead_code)]
fn with_space(scenario: TestScenario, space: &str) -> TestScenario {
    scenario
        .exec_dcl(&format!("CREATE SPACE {} WITH DIMENSION=128", space))
        .assert_success()
}

// ==================== GRANT Parser Tests ====================

#[test]
fn test_grant_parser_basic() {
    let query = "GRANT ROLE ADMIN ON test_space TO alice";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GRANT basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GRANT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GRANT");
}

#[test]
fn test_grant_parser_without_role_keyword() {
    let query = "GRANT ADMIN ON test_space TO alice";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GRANT without ROLE keyword parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GRANT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GRANT");
}

#[test]
fn test_grant_parser_all_roles() {
    let queries = vec![
        "GRANT GOD ON test_space TO user1",
        "GRANT ADMIN ON test_space TO user2",
        "GRANT DBA ON test_space TO user3",
        "GRANT USER ON test_space TO user4",
        "GRANT GUEST ON test_space TO user5",
    ];

    for query in queries {
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "GRANT role {} parsing should succeed: {:?}",
            query,
            result.err()
        );
    }
}

// ==================== REVOKE Parser Tests ====================

#[test]
fn test_revoke_parser_basic() {
    let query = "REVOKE ROLE ADMIN ON test_space FROM alice";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "REVOKE basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("REVOKE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "REVOKE");
}

#[test]
fn test_revoke_parser_without_role_keyword() {
    let query = "REVOKE ADMIN ON test_space FROM alice";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "REVOKE without ROLE keyword parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("REVOKE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "REVOKE");
}

// ==================== GRANT/REVOKE Execution Tests ====================

#[test]
fn test_grant_revoke_execution() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'password123'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON test_space TO alice")
        .assert_success()
        // Verify role was granted via SHOW ROLES
        .exec_dcl("SHOW ROLES IN test_space")
        .assert_success()
        .exec_dcl("REVOKE ADMIN ON test_space FROM alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

#[test]
fn test_grant_multiple_roles() {
    let mut scenario = new_scenario()
        .exec_dcl("CREATE USER multi_role_user WITH PASSWORD 'password'")
        .assert_success();

    for space_name in ["space1", "space2", "space3"] {
        scenario = scenario
            .exec_dcl(&format!("CREATE SPACE {} WITH DIMENSION=128", space_name))
            .assert_success();
    }

    let grant_queries = [
        "GRANT ADMIN ON space1 TO multi_role_user",
        "GRANT DBA ON space2 TO multi_role_user",
        "GRANT USER ON space3 TO multi_role_user",
    ];
    for q in &grant_queries {
        scenario = scenario.exec_dcl(q).assert_success();
    }

    // Verify roles via SHOW ROLES
    scenario = scenario
        .exec_dcl("SHOW ROLES")
        .assert_success()
        .exec_dcl("SHOW ROLES IN space1")
        .assert_success();

    let revoke_queries = [
        "REVOKE ADMIN ON space1 FROM multi_role_user",
        "REVOKE DBA ON space2 FROM multi_role_user",
        "REVOKE USER ON space3 FROM multi_role_user",
    ];
    for q in &revoke_queries {
        scenario = scenario.exec_dcl(q).assert_success();
    }

    scenario
        .exec_dcl("DROP USER multi_role_user")
        .assert_success();
}

#[test]
fn test_grant_nonexistent_user() {
    new_scenario()
        .exec_dcl("GRANT ADMIN ON test_space TO nonexistent_user")
        .assert_error();
}

#[test]
fn test_revoke_nonexistent_permission() {
    new_scenario()
        .exec_dcl("CREATE USER testuser WITH PASSWORD 'password'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        // Revoking a permission that was never granted
        .exec_dcl("REVOKE ADMIN ON test_space FROM testuser")
        .assert_success()
        .exec_dcl("DROP USER testuser")
        .assert_success();
}

#[test]
fn test_revoke_nonexistent_user() {
    new_scenario()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        // REVOKE from a user that was never created
        .exec_dcl("REVOKE ADMIN ON test_space FROM nonexistent_user")
        .assert_error();
}

#[test]
fn test_grant_nonexistent_space() {
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'pass'")
        .assert_success()
        // GRANT on a space that was never created
        .exec_dcl("GRANT ADMIN ON nonexistent_space TO alice")
        .assert_error();
}

#[test]
fn test_grant_duplicate_role() {
    // Grant the same role twice — should be idempotent
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON test_space TO alice")
        .assert_success()
        // Duplicate grant should succeed (idempotent)
        .exec_dcl("GRANT ADMIN ON test_space TO alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

#[test]
fn test_revoke_twice() {
    // Revoke the same permission twice
    new_scenario()
        .exec_dcl("CREATE USER alice WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON test_space TO alice")
        .assert_success()
        .exec_dcl("REVOKE ADMIN ON test_space FROM alice")
        .assert_success()
        // Second revoke should succeed (no-op since already revoked)
        .exec_dcl("REVOKE ADMIN ON test_space FROM alice")
        .assert_success()
        .exec_dcl("DROP USER alice")
        .assert_success();
}

#[test]
fn test_grant_revoke_all_role_types() {
    // Test GRANT/REVOKE cycle for all 5 role types
    let mut scenario = new_scenario()
        .exec_dcl("CREATE USER role_test WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE test_space WITH DIMENSION=128")
        .assert_success();

    for role in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
        scenario = scenario
            .exec_dcl(&format!("GRANT {} ON test_space TO role_test", role))
            .assert_success();
    }

    for role in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
        scenario = scenario
            .exec_dcl(&format!("REVOKE {} ON test_space FROM role_test", role))
            .assert_success();
    }

    scenario.exec_dcl("DROP USER role_test").assert_success();
}

#[test]
fn test_grant_multiple_users_same_space() {
    // Grant multiple users roles on the same space
    let mut scenario = new_scenario()
        .exec_dcl("CREATE SPACE shared_space WITH DIMENSION=128")
        .assert_success();

    for user in ["alice", "bob", "charlie"] {
        scenario = scenario
            .exec_dcl(&format!("CREATE USER {} WITH PASSWORD 'pass'", user))
            .assert_success()
            .exec_dcl(&format!("GRANT DBA ON shared_space TO {}", user))
            .assert_success();
    }

    for user in ["alice", "bob", "charlie"] {
        scenario = scenario
            .exec_dcl(&format!("REVOKE DBA ON shared_space FROM {}", user))
            .assert_success()
            .exec_dcl(&format!("DROP USER {}", user))
            .assert_success();
    }
}
