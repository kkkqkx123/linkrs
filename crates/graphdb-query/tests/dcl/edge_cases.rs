//! Edge Cases and Boundary Conditions DCL Tests
//!
//! Test edge cases and boundary conditions:
//! - Extreme values (very long strings, special characters)
//! - Unusual combinations
//! - Rare scenarios

use super::common;
use common::test_scenario::TestScenario;

fn new_scenario() -> TestScenario {
    TestScenario::new().expect("Failed to create test scenario")
}

// ==================== Length Boundary Tests ====================

#[test]
fn test_username_max_length_boundary() {
    let max_len_username = "u".repeat(255);
    let query = format!("CREATE USER {} WITH PASSWORD 'pass'", max_len_username);

    let scenario = new_scenario();
    let result = scenario.exec_dcl(&query);
    // Should either accept or reject gracefully
    result;
}

#[test]
fn test_username_one_char() {
    new_scenario()
        .exec_dcl("CREATE USER x WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER x")
        .assert_success();
}

#[test]
fn test_password_very_long() {
    let long_password = "p".repeat(1000);
    let query = format!("CREATE USER longpwd_user WITH PASSWORD '{}'", long_password);

    new_scenario()
        .exec_dcl(&query)
        .assert_success()
        .exec_dcl("DROP USER longpwd_user")
        .assert_success();
}

#[test]
fn test_space_name_very_long() {
    let long_space = "s".repeat(255);
    let query = format!("CREATE SPACE {} WITH DIMENSION=128", long_space);

    let scenario = new_scenario()
        .exec_dcl(&query)
        .assert_success()
        .exec_dcl("CREATE USER long_space_user WITH PASSWORD 'pass'")
        .assert_success();

    scenario
        .exec_dcl(&format!("GRANT ADMIN ON {} TO long_space_user", long_space))
        .assert_success();
}

// ==================== Special Character Tests ====================

#[test]
fn test_username_with_numbers() {
    new_scenario()
        .exec_dcl("CREATE USER user123 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER user123")
        .assert_success();
}

#[test]
fn test_username_with_underscores() {
    new_scenario()
        .exec_dcl("CREATE USER user_name_123 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER user_name_123")
        .assert_success();
}

#[test]
fn test_username_with_hyphens() {
    new_scenario()
        .exec_dcl("CREATE USER user-name-123 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER user-name-123")
        .assert_success();
}

#[test]
fn test_password_all_special_chars() {
    let special_pwd = "!@#$%^&*()_+-=[]{}|;:',.<>?/~`";
    let query = format!("CREATE USER special_pwd WITH PASSWORD '{}'", special_pwd);

    new_scenario()
        .exec_dcl(&query)
        .assert_success()
        .exec_dcl("DROP USER special_pwd")
        .assert_success();
}

#[test]
fn test_password_with_quotes_escaped() {
    new_scenario()
        .exec_dcl("CREATE USER quote_pwd WITH PASSWORD 'pass\"word'")
        .assert_success()
        .exec_dcl("DROP USER quote_pwd")
        .assert_success();
}

#[test]
fn test_password_with_newline_like_chars() {
    new_scenario()
        .exec_dcl("CREATE USER newline_pwd WITH PASSWORD 'pass\\nword'")
        .assert_success()
        .exec_dcl("DROP USER newline_pwd")
        .assert_success();
}

// ==================== Case Sensitivity Tests ====================

#[test]
fn test_username_case_differences() {
    new_scenario()
        .exec_dcl("CREATE USER CaseSensitive WITH PASSWORD 'pass'")
        .assert_success()
        // Different case should not match
        .exec_dcl("DESCRIBE USER casesensitive")
        .assert_error()
        .exec_dcl("DROP USER CaseSensitive")
        .assert_success();
}

#[test]
fn test_role_case_insensitivity() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER role_case WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE role_case_space WITH DIMENSION=128")
        .assert_success();

    scenario
        // All case variations should work
        .exec_dcl("GRANT admin ON role_case_space TO role_case")
        .assert_success()
        .exec_dcl("REVOKE admin ON role_case_space FROM role_case")
        .assert_success()
        .exec_dcl("GRANT ADMIN ON role_case_space TO role_case")
        .assert_success()
        .exec_dcl("REVOKE ADMIN ON role_case_space FROM role_case")
        .assert_success()
        .exec_dcl("GRANT Admin ON role_case_space TO role_case")
        .assert_success();
}

// ==================== Unicode and Encoding Tests ====================

#[test]
fn test_username_chinese_chars() {
    new_scenario()
        .exec_dcl("CREATE USER 用户名 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER 用户名")
        .assert_success();
}

#[test]
fn test_username_mixed_unicode() {
    new_scenario()
        .exec_dcl("CREATE USER user_用户_名 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER user_用户_名")
        .assert_success();
}

#[test]
fn test_password_emoji() {
    new_scenario()
        .exec_dcl("CREATE USER emoji_pwd WITH PASSWORD '🔐🔑🗝️'")
        .assert_success()
        .exec_dcl("DROP USER emoji_pwd")
        .assert_success();
}

#[test]
fn test_unicode_space_name() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER unicode_space_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE 空间 WITH DIMENSION=128")
        .assert_success();

    scenario
        .exec_dcl("GRANT ADMIN ON 空间 TO unicode_space_user")
        .assert_success()
        .exec_dcl("SHOW ROLES IN 空间")
        .assert_success();
}

// ==================== Reserved Keywords As Names ====================

#[test]
fn test_reserved_keyword_as_username() {
    let keywords = vec![
        "CREATE", "DROP", "ALTER", "SELECT", "DELETE", "INSERT", "UPDATE", "GRANT", "REVOKE",
    ];

    for keyword in keywords {
        let query = format!("CREATE USER {} WITH PASSWORD 'pass'", keyword);
        let scenario = new_scenario();
        let result = scenario.exec_dcl(&query);
        // Should either accept (as literal) or reject gracefully
        result;
    }
}

#[test]
fn test_reserved_keyword_as_space_name() {
    let query = "CREATE SPACE CREATE WITH DIMENSION=128";
    let scenario = new_scenario();
    let result = scenario.exec_dcl(query);
    // Should either accept or reject gracefully
    result;
}

// ==================== Whitespace Handling ====================

#[test]
fn test_leading_trailing_spaces_in_values() {
    // Exact behavior depends on parser, but should handle gracefully
    let scenario = new_scenario();
    let result = scenario.exec_dcl("CREATE USER \" user \" WITH PASSWORD 'pass'");
    result;
}

#[test]
fn test_multiple_spaces_in_query() {
    new_scenario()
        .exec_dcl("CREATE    USER    multispace    WITH    PASSWORD    'pass'")
        .assert_success()
        .exec_dcl("DROP USER multispace")
        .assert_success();
}

#[test]
fn test_tab_characters_in_query() {
    new_scenario()
        .exec_dcl("CREATE\tUSER\ttabuser\tWITH\tPASSWORD\t'pass'")
        .assert_success()
        .exec_dcl("DROP USER tabuser")
        .assert_success();
}

// ==================== Numeric Boundary Tests ====================

#[test]
fn test_numeric_only_username() {
    new_scenario()
        .exec_dcl("CREATE USER 12345 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER 12345")
        .assert_success();
}

#[test]
fn test_very_large_numeric_username() {
    new_scenario()
        .exec_dcl("CREATE USER 99999999999999999 WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER 99999999999999999")
        .assert_success();
}

// ==================== Empty and Null-Like Values ====================

#[test]
fn test_null_as_literal_username() {
    new_scenario()
        .exec_dcl("CREATE USER NULL WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("DROP USER NULL")
        .assert_success();
}

#[test]
fn test_empty_string_username() {
    new_scenario()
        .exec_dcl("CREATE USER \"\" WITH PASSWORD 'pass'")
        .assert_error();
}

// ==================== Repeated Operations ====================

#[test]
fn test_create_drop_cycle() {
    let scenario = new_scenario();

    let mut scenario = scenario;
    for i in 0..5 {
        scenario = scenario
            .exec_dcl(&format!(
                "CREATE USER cycle_user_{} WITH PASSWORD 'pass'",
                i
            ))
            .assert_success()
            .exec_dcl(&format!("DROP USER cycle_user_{}", i))
            .assert_success();
    }
}

#[test]
fn test_grant_revoke_cycle() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER cycle_perm_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE cycle_perm_space WITH DIMENSION=128")
        .assert_success();

    let mut scenario = scenario;
    for _ in 0..5 {
        scenario = scenario
            .exec_dcl("GRANT ADMIN ON cycle_perm_space TO cycle_perm_user")
            .assert_success()
            .exec_dcl("REVOKE ADMIN ON cycle_perm_space FROM cycle_perm_user")
            .assert_success();
    }
}

#[test]
fn test_password_change_cycle() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER pwd_cycle_user WITH PASSWORD 'pass0'")
        .assert_success();

    let mut scenario = scenario;
    for i in 1..5 {
        scenario = scenario
            .exec_dcl(&format!(
                "CHANGE PASSWORD pwd_cycle_user 'pass{}' TO 'pass{}'",
                i - 1,
                i
            ))
            .assert_success();
    }
}

// ==================== Combination Tests ====================

#[test]
fn test_all_operations_same_user() {
    let scenario = new_scenario();

    scenario
        // Create
        .exec_dcl("CREATE USER all_ops WITH PASSWORD 'pass0'")
        .assert_success()
        // Describe
        .exec_dcl("DESCRIBE USER all_ops")
        .assert_success()
        // Alter (change password)
        .exec_dcl("ALTER USER all_ops WITH PASSWORD 'pass1'")
        .assert_success()
        // Create space
        .exec_dcl("CREATE SPACE all_ops_space WITH DIMENSION=128")
        .assert_success()
        // Grant
        .exec_dcl("GRANT ADMIN ON all_ops_space TO all_ops")
        .assert_success()
        // Show roles
        .exec_dcl("SHOW ROLES IN all_ops_space")
        .assert_success()
        // Change password again
        .exec_dcl("CHANGE PASSWORD all_ops 'pass1' TO 'pass2'")
        .assert_success()
        // Revoke
        .exec_dcl("REVOKE ADMIN ON all_ops_space FROM all_ops")
        .assert_success()
        // Describe again
        .exec_dcl("DESCRIBE USER all_ops")
        .assert_success()
        // Drop
        .exec_dcl("DROP USER all_ops")
        .assert_success();
}

#[test]
fn test_all_roles_in_sequence() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER all_roles_user WITH PASSWORD 'pass'")
        .assert_success()
        .exec_dcl("CREATE SPACE all_roles_space WITH DIMENSION=128")
        .assert_success();

    let mut scenario = scenario;
    for role in ["GOD", "ADMIN", "DBA", "USER", "GUEST"] {
        scenario = scenario
            .exec_dcl(&format!(
                "GRANT {} ON all_roles_space TO all_roles_user",
                role
            ))
            .assert_success()
            .exec_dcl(&format!(
                "REVOKE {} ON all_roles_space FROM all_roles_user",
                role
            ))
            .assert_success();
    }
}

// ==================== Recovery Tests ====================

#[test]
fn test_recover_from_failed_operations() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER recovery_user WITH PASSWORD 'pass'")
        .assert_success();

    scenario
        // Attempt invalid operation
        .exec_dcl("CREATE USER recovery_user WITH INVALID_SYNTAX")
        .assert_error()
        // User should still be accessible
        .exec_dcl("DESCRIBE USER recovery_user")
        .assert_success()
        // Can continue normal operations
        .exec_dcl("DROP USER recovery_user")
        .assert_success();
}

#[test]
fn test_recover_from_multiple_failures() {
    let scenario = new_scenario()
        .exec_dcl("CREATE USER multi_fail_user WITH PASSWORD 'pass'")
        .assert_success();

    scenario
        // Multiple failures
        .exec_dcl("ALTER USER nonexistent WITH PASSWORD 'pass'")
        .assert_error()
        .exec_dcl("DROP USER nonexistent")
        .assert_error()
        .exec_dcl("GRANT INVALID ON space TO user")
        .assert_error()
        // Original user still accessible
        .exec_dcl("DESCRIBE USER multi_fail_user")
        .assert_success();
}
