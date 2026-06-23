//! Integration testing of permission management
//!
//! Test scope:
//! PermissionManager – Core functions of the permission manager
//! PermissionChecker – Permission checker
//! Authenticator – Authenticator
//! Role Granting and Revoking (GRANT/REVOKE)
//! Permission check scenarios

mod common;

use std::sync::Arc;

use graphdb::api::server::permission::OperationType;
use graphdb::api::server::permission::GOD_SPACE_ID;
use graphdb::api::server::session::ClientSession;
use graphdb::api::server::{
    Authenticator, PasswordAuthenticator, Permission, PermissionChecker, PermissionManager,
    RoleType, Session,
};
use graphdb::config::AuthConfig;
use graphdb::core::types::SpaceInfo;

// ==================== PermissionManager 核心测试 ====================

#[test]
fn test_permission_manager_creation() {
    let pm = PermissionManager::new();

    // The root user should automatically become the “God” role.
    assert!(
        pm.is_god("root"),
        "The root user should be considered the “God” character."
    );
    assert!(
        pm.is_admin("root"),
        "The root user should have the Admin role."
    );
}

#[test]
fn test_grant_and_revoke_role() {
    let pm = PermissionManager::new();
    let space_id = 1i64;

    // Grant the User role
    pm.grant_role("user1", space_id, RoleType::User)
        .expect("授予User角色应该成功");

    // Verify that the role has been granted.
    let role = pm.get_role("user1", space_id);
    assert_eq!(
        role,
        Some(RoleType::User),
        "The User role should be obtained."
    );

    // Revoke the role
    pm.revoke_role("user1", space_id).expect("撤销角色应该成功");

    // Verification that the role has been revoked.
    let role_after = pm.get_role("user1", space_id);
    assert_eq!(role_after, None, "The role should have been revoked.");
}

#[test]
fn test_grant_multiple_roles_to_user() {
    let pm = PermissionManager::new();

    // Granting different roles to users in different spaces
    pm.grant_role("multi_role_user", 1, RoleType::Admin)
        .expect("授予Admin角色应该成功");
    pm.grant_role("multi_role_user", 2, RoleType::User)
        .expect("授予User角色应该成功");
    pm.grant_role("multi_role_user", 3, RoleType::Guest)
        .expect("授予Guest角色应该成功");

    // Verify the roles of each member in the Space.
    assert_eq!(pm.get_role("multi_role_user", 1), Some(RoleType::Admin));
    assert_eq!(pm.get_role("multi_role_user", 2), Some(RoleType::User));
    assert_eq!(pm.get_role("multi_role_user", 3), Some(RoleType::Guest));

    // Testing the list_user_roles method
    let user_roles = pm.list_user_roles("multi_role_user");
    assert_eq!(user_roles.len(), 3, "Users should have 3 roles.");
}

#[test]
fn test_list_space_users() {
    let pm = PermissionManager::new();
    let space_id = 1i64;

    // Granting roles to multiple users
    pm.grant_role("user1", space_id, RoleType::User)
        .expect("授予User角色应该成功");
    pm.grant_role("user2", space_id, RoleType::Admin)
        .expect("授予Admin角色应该成功");
    pm.grant_role("user3", space_id, RoleType::Guest)
        .expect("授予Guest角色应该成功");

    // Testing the list_space_users method
    let space_users = pm.list_space_users(space_id);
    assert_eq!(
        space_users.len(),
        3,
        "There should be 3 users in the Space."
    );

    // Verify that the list contains the correct users.
    let usernames: Vec<String> = space_users.iter().map(|(name, _)| name.clone()).collect();
    assert!(usernames.contains(&"user1".to_string()));
    assert!(usernames.contains(&"user2".to_string()));
    assert!(usernames.contains(&"user3".to_string()));
}

// ==================== Role Permission Check Test ====================

#[test]
fn test_god_role_has_all_permissions() {
    let pm = PermissionManager::new();
    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);
    let session = create_client_session_with_role("root", 0, RoleType::God);

    // The God character possesses all permissions.
    assert!(RoleType::God.has_permission(Permission::Read));
    assert!(RoleType::God.has_permission(Permission::Write));
    assert!(RoleType::God.has_permission(Permission::Delete));
    assert!(RoleType::God.has_permission(Permission::Schema));
    assert!(RoleType::God.has_permission(Permission::Admin));

    // God can access any Space.
    assert!(checker.can_read_space(&session, 1).is_ok());
    assert!(checker.can_read_space(&session, 999).is_ok());

    // God can be written into Space.
    assert!(checker.can_write_space(&session).is_ok());

    // God can be written into the Schema.
    assert!(checker.can_write_schema(&session, 1).is_ok());
}

#[test]
fn test_admin_role_permissions() {
    let pm = PermissionManager::new();
    let space_id = 1i64;

    pm.grant_role("admin1", space_id, RoleType::Admin)
        .expect("授予Admin角色应该成功");

    // The admin has all the permissions.
    assert!(pm
        .check_permission("admin1", space_id, Permission::Read)
        .is_ok());
    assert!(pm
        .check_permission("admin1", space_id, Permission::Write)
        .is_ok());
    assert!(pm
        .check_permission("admin1", space_id, Permission::Delete)
        .is_ok());
    assert!(pm
        .check_permission("admin1", space_id, Permission::Schema)
        .is_ok());
    assert!(pm
        .check_permission("admin1", space_id, Permission::Admin)
        .is_ok());
}

#[test]
fn test_dba_role_permissions() {
    let pm = PermissionManager::new();
    let space_id = 1i64;

    pm.grant_role("dba1", space_id, RoleType::Dba)
        .expect("授予Dba角色应该成功");

    // The user DBA has permissions to read, write, delete data, as well as to modify the database schema (the structure of the database).
    assert!(pm
        .check_permission("dba1", space_id, Permission::Read)
        .is_ok());
    assert!(pm
        .check_permission("dba1", space_id, Permission::Write)
        .is_ok());
    assert!(pm
        .check_permission("dba1", space_id, Permission::Delete)
        .is_ok());
    assert!(pm
        .check_permission("dba1", space_id, Permission::Schema)
        .is_ok());

    // The user DBA does not have Admin privileges.
    assert!(pm
        .check_permission("dba1", space_id, Permission::Admin)
        .is_err());
}

#[test]
fn test_user_role_permissions() {
    let pm = PermissionManager::new();
    let space_id = 1i64;

    pm.grant_role("user1", space_id, RoleType::User)
        .expect("授予User角色应该成功");

    // The user has the permissions to read, write, and delete content.
    assert!(pm
        .check_permission("user1", space_id, Permission::Read)
        .is_ok());
    assert!(pm
        .check_permission("user1", space_id, Permission::Write)
        .is_ok());
    assert!(pm
        .check_permission("user1", space_id, Permission::Delete)
        .is_ok());

    // The user does not have the Schema or Admin permissions.
    assert!(pm
        .check_permission("user1", space_id, Permission::Schema)
        .is_err());
    assert!(pm
        .check_permission("user1", space_id, Permission::Admin)
        .is_err());
}

#[test]
fn test_guest_role_permissions() {
    let pm = PermissionManager::new();
    let space_id = 1i64;

    pm.grant_role("guest1", space_id, RoleType::Guest)
        .expect("授予Guest角色应该成功");

    // Guests only have read access.
    assert!(pm
        .check_permission("guest1", space_id, Permission::Read)
        .is_ok());

    // The guest does not have the permissions to write, delete, modify the schema, or access administrative functions.
    assert!(pm
        .check_permission("guest1", space_id, Permission::Write)
        .is_err());
    assert!(pm
        .check_permission("guest1", space_id, Permission::Delete)
        .is_err());
    assert!(pm
        .check_permission("guest1", space_id, Permission::Schema)
        .is_err());
    assert!(pm
        .check_permission("guest1", space_id, Permission::Admin)
        .is_err());
}

// ==================== Role Authorization Testing ====================

#[test]
fn test_god_can_grant_any_role() {
    let pm = PermissionManager::new();
    let space_id = 1i64;
    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);
    let session = create_client_session_with_role("root", 0, RoleType::God);

    // God can grant any role to anyone.
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::God)
        .is_ok());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Admin)
        .is_ok());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Dba)
        .is_ok());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::User)
        .is_ok());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Guest)
        .is_ok());
}

#[test]
fn test_admin_grant_role_permissions() {
    let pm = PermissionManager::new();
    let space_id = 1i64;
    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    checker
        .permission_manager()
        .grant_role("admin1", space_id, RoleType::Admin)
        .expect("授予Admin角色应该成功");
    let session = create_client_session_with_role("admin1", space_id, RoleType::Admin);

    // The admin can grant roles to DBAs, users, and guests.
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Dba)
        .is_ok());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::User)
        .is_ok());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Guest)
        .is_ok());

    // The Admin cannot grant privileges to God or another Admin.
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::God)
        .is_err());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Admin)
        .is_err());
}

#[test]
fn test_dba_grant_role_permissions() {
    let pm = PermissionManager::new();
    let space_id = 1i64;
    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    checker
        .permission_manager()
        .grant_role("dba1", space_id, RoleType::Dba)
        .expect("授予Dba角色应该成功");
    let session = create_client_session_with_role("dba1", space_id, RoleType::Dba);

    // The DBA can grant privileges to Users and Guests.
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::User)
        .is_ok());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Guest)
        .is_ok());

    // The “Dba” role cannot be granted to “God”, “Admin”, or “Dba”.
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::God)
        .is_err());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Admin)
        .is_err());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Dba)
        .is_err());
}

#[test]
fn test_user_cannot_grant_any_role() {
    let pm = PermissionManager::new();
    let space_id = 1i64;
    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    checker
        .permission_manager()
        .grant_role("user1", space_id, RoleType::User)
        .expect("授予User角色应该成功");
    let session = create_client_session_with_role("user1", space_id, RoleType::User);

    // Users cannot assign any roles.
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::User)
        .is_err());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Guest)
        .is_err());
    assert!(checker
        .can_write_role(&session, space_id, "target_user", RoleType::Dba)
        .is_err());
}

#[test]
fn test_cannot_modify_own_role() {
    let pm = PermissionManager::new();
    let space_id = 1i64;
    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    checker
        .permission_manager()
        .grant_role("admin1", space_id, RoleType::Admin)
        .expect("授予Admin角色应该成功");
    let session = create_client_session_with_role("admin1", space_id, RoleType::Admin);

    // You cannot change your own role.
    assert!(checker
        .can_write_role(&session, space_id, "admin1", RoleType::User)
        .is_err());
}

// ==================== PermissionChecker 测试 ====================

fn create_test_config() -> AuthConfig {
    AuthConfig {
        enable_authorize: true,
        failed_login_attempts: 5,
        session_idle_timeout_secs: 3600,
        default_username: "root".to_string(),
        default_password: "root".to_string(),
        force_change_default_password: true,
    }
}

fn create_test_session(username: &str) -> Session {
    Session {
        session_id: 1,
        user_name: username.to_string(),
        space_name: None,
        graph_addr: Some("127.0.0.1:1234".to_string()),
        timezone: None,
    }
}

fn create_client_session_with_role(
    username: &str,
    space_id: i64,
    role: RoleType,
) -> Arc<ClientSession> {
    let session = create_test_session(username);
    let client_session = ClientSession::new(session);
    client_session.set_role(space_id, role);
    client_session
}

#[test]
fn test_permission_checker_with_disabled_auth() {
    let pm = PermissionManager::new();
    let mut config = create_test_config();
    config.enable_authorize = false; // Disable authorization

    let checker = PermissionChecker::new(pm, config);
    let session = create_client_session_with_role("user1", 1, RoleType::User);

    // When authorization is disabled, all checks should pass.
    assert!(checker.can_read_space(&session, 1).is_ok());
    assert!(checker.can_write_space(&session).is_ok());
    assert!(checker.can_write_schema(&session, 1).is_ok());
}

#[test]
fn test_permission_checker_space_operations() {
    let pm = PermissionManager::new();
    pm.grant_role("user1", 1, RoleType::User)
        .expect("授予User角色应该成功");
    pm.grant_role("admin1", 1, RoleType::Admin)
        .expect("授予Admin角色应该成功");

    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    let user_session = create_client_session_with_role("user1", 1, RoleType::User);
    let admin_session = create_client_session_with_role("admin1", 1, RoleType::Admin);

    // Users can access the allocated space.
    assert!(checker.can_read_space(&user_session, 1).is_ok());
    // Users cannot access (read or use) unallocated space.
    assert!(checker.can_read_space(&user_session, 2).is_err());

    // Users are not allowed to create new spaces (i.e., to write content into these spaces).
    assert!(checker.can_write_space(&user_session).is_err());
    // Even the admin cannot write in the “Space” area (only God can do that).
    assert!(checker.can_write_space(&admin_session).is_err());
}

#[test]
fn test_permission_checker_schema_operations() {
    let pm = PermissionManager::new();
    pm.grant_role("admin1", 1, RoleType::Admin)
        .expect("授予Admin角色应该成功");
    pm.grant_role("user1", 1, RoleType::User)
        .expect("授予User角色应该成功");

    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    let admin_session = create_client_session_with_role("admin1", 1, RoleType::Admin);
    let user_session = create_client_session_with_role("user1", 1, RoleType::User);

    // Admin can read and write Schema
    assert!(checker.can_read_schema(&admin_session, 1).is_ok());
    assert!(checker.can_write_schema(&admin_session, 1).is_ok());

    // User can read Schema but cannot write it
    assert!(checker.can_read_schema(&user_session, 1).is_ok());
    assert!(checker.can_write_schema(&user_session, 1).is_err());
}

#[test]
fn test_permission_checker_data_operations() {
    let pm = PermissionManager::new();
    pm.grant_role("user1", 1, RoleType::User)
        .expect("授予User角色应该成功");
    pm.grant_role("guest1", 1, RoleType::Guest)
        .expect("授予Guest角色应该成功");

    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    let user_session = create_client_session_with_role("user1", 1, RoleType::User);
    let guest_session = create_client_session_with_role("guest1", 1, RoleType::Guest);

    // User can read and write data
    assert!(checker.can_read_data(&user_session, 1).is_ok());
    assert!(checker.can_write_data(&user_session, 1).is_ok());

    // Guest can only read data
    assert!(checker.can_read_data(&guest_session, 1).is_ok());
    assert!(checker.can_write_data(&guest_session, 1).is_err());
}

#[test]
fn test_permission_checker_user_operations() {
    let pm = PermissionManager::new();
    pm.grant_role("admin1", 1, RoleType::Admin)
        .expect("授予Admin角色应该成功");

    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    let god_session = create_client_session_with_role("root", GOD_SPACE_ID, RoleType::God);
    let admin_session = create_client_session_with_role("admin1", 1, RoleType::Admin);

    // God can manage users
    assert!(checker.can_write_user(&god_session).is_ok());
    assert!(checker.can_read_user(&god_session, "anyuser").is_ok());

    // Admin can't manage users (only God can)
    assert!(checker.can_write_user(&admin_session).is_err());

    // Admin can read their own information
    assert!(checker.can_read_user(&admin_session, "admin1").is_ok());
    // Admin can read information about other users (Admin is the space administrator)
    assert!(checker.can_read_user(&admin_session, "otheruser").is_ok());
}

#[test]
fn test_permission_checker_role_operations() {
    let pm = PermissionManager::new();
    pm.grant_role("admin1", 1, RoleType::Admin)
        .expect("授予Admin角色应该成功");

    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    let god_session = create_client_session_with_role("root", GOD_SPACE_ID, RoleType::God);
    let admin_session = create_client_session_with_role("admin1", 1, RoleType::Admin);

    // God can be granted to any role
    assert!(checker
        .can_write_role(&god_session, 1, "target_user", RoleType::Admin)
        .is_ok());

    // Admin can grant certain roles
    assert!(checker
        .can_write_role(&admin_session, 1, "target_user", RoleType::User)
        .is_ok());
    assert!(checker
        .can_write_role(&admin_session, 1, "target_user", RoleType::Guest)
        .is_ok());

    // Admin cannot grant God
    assert!(checker
        .can_write_role(&admin_session, 1, "target_user", RoleType::God)
        .is_err());

    // Admin can't modify his role
    assert!(checker
        .can_write_role(&admin_session, 1, "admin1", RoleType::User)
        .is_err());
}

#[test]
fn test_permission_checker_change_password() {
    let pm = PermissionManager::new();
    pm.grant_role("user1", 1, RoleType::User)
        .expect("授予User角色应该成功");

    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    let god_session = create_client_session_with_role("root", GOD_SPACE_ID, RoleType::God);
    let user_session = create_client_session_with_role("user1", 1, RoleType::User);

    // Users can change their passwords
    assert!(checker
        .check_permission(
            &user_session,
            OperationType::ChangePassword,
            None,
            Some("user1"),
            None
        )
        .is_ok());

    // Users cannot change other users' passwords
    assert!(checker
        .check_permission(
            &user_session,
            OperationType::ChangePassword,
            None,
            Some("otheruser"),
            None
        )
        .is_err());

    // God can change any user's password
    assert!(checker
        .check_permission(
            &god_session,
            OperationType::ChangePassword,
            None,
            Some("anyuser"),
            None
        )
        .is_ok());
}

#[test]
fn test_permission_checker_show_operation() {
    let pm = PermissionManager::new();
    pm.grant_role("guest1", 1, RoleType::Guest)
        .expect("授予Guest角色应该成功");

    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);

    let guest_session = create_client_session_with_role("guest1", 1, RoleType::Guest);

    // The SHOW operation usually allows all users to
    assert!(checker
        .check_permission(&guest_session, OperationType::Show, None, None, None)
        .is_ok());
}

// ==================== Authenticator 测试 ====================

#[test]
fn test_password_authenticator_success() {
    let config = create_test_config();
    let auth = PasswordAuthenticator::new(|_username: &str, _password: &str| Ok(true), config);

    assert!(auth.authenticate("user", "pass").is_ok());
}

#[test]
fn test_password_authenticator_failure() {
    let config = create_test_config();
    let auth = PasswordAuthenticator::new(|_username: &str, _password: &str| Ok(false), config);

    assert!(auth.authenticate("user", "wrong_pass").is_err());
}

#[test]
fn test_password_authenticator_default() {
    let config = AuthConfig {
        enable_authorize: true,
        failed_login_attempts: 0, // Disable Login Restrictions
        session_idle_timeout_secs: 3600,
        default_username: "admin".to_string(),
        default_password: "admin123".to_string(),
        force_change_default_password: false,
    };

    let auth = PasswordAuthenticator::new_default(config);

    // Use the correct default credentials
    assert!(auth.authenticate("admin", "admin123").is_ok());

    // Using the wrong credentials
    assert!(auth.authenticate("admin", "wrong").is_err());
}

#[test]
fn test_password_authenticator_empty_credentials() {
    let config = create_test_config();
    let auth = PasswordAuthenticator::new(|_username: &str, _password: &str| Ok(true), config);

    // empty username
    assert!(auth.authenticate("", "pass").is_err());

    // empty password
    assert!(auth.authenticate("user", "").is_err());

    // They're all empty.
    assert!(auth.authenticate("", "").is_err());
}

#[test]
fn test_password_authenticator_disabled() {
    let mut config = create_test_config();
    config.enable_authorize = false; // disable authorization

    let auth = PasswordAuthenticator::new(
        |_username: &str, _password: &str| Ok(false), // Even if the validator returns false
        config,
    );

    // When authorization is disabled, any credentials should be passed through the
    assert!(auth.authenticate("any", "any").is_ok());
}

#[test]
fn test_password_authenticator_login_attempts_limit() {
    let config = AuthConfig {
        enable_authorize: true,
        failed_login_attempts: 3, // Maximum 3 attempts
        session_idle_timeout_secs: 3600,
        default_username: "root".to_string(),
        default_password: "root".to_string(),
        force_change_default_password: false,
    };

    let auth = PasswordAuthenticator::new(|_username: &str, _password: &str| Ok(false), config);

    // First failure.
    let result1 = auth.authenticate("user", "wrong");
    assert!(result1.is_err());
    assert!(result1
        .unwrap_err()
        .to_string()
        .contains("还剩 2 次尝试机会"));

    // Second failure
    let result2 = auth.authenticate("user", "wrong");
    assert!(result2.is_err());
    assert!(result2
        .unwrap_err()
        .to_string()
        .contains("还剩 1 次尝试机会"));

    // Third failure
    let result3 = auth.authenticate("user", "wrong");
    assert!(result3.is_err());
    assert!(result3
        .unwrap_err()
        .to_string()
        .contains("已达到最大尝试次数"));
}

// ==================== ClientSession 角色管理测试 ====================

#[test]
fn test_client_session_role_management() {
    let session = create_test_session("testuser");
    let client_session = ClientSession::new(session);

    // No character at the beginning
    assert!(!client_session.is_god());
    assert!(!client_session.is_admin());
    assert_eq!(client_session.role_with_space(1), None);

    // Setting up the role
    client_session.set_role(1, RoleType::User);
    assert_eq!(client_session.role_with_space(1), Some(RoleType::User));

    // Setting up the God role
    client_session.set_role(GOD_SPACE_ID, RoleType::God);
    assert!(client_session.is_god());
    assert!(client_session.is_admin());
}

#[test]
fn test_client_session_multiple_spaces() {
    let session = create_test_session("testuser");
    let client_session = ClientSession::new(session);

    // Setting up different roles in different Space
    client_session.set_role(1, RoleType::Admin);
    client_session.set_role(2, RoleType::User);
    client_session.set_role(3, RoleType::Guest);

    // Validate the roles of each Space
    assert_eq!(client_session.role_with_space(1), Some(RoleType::Admin));
    assert_eq!(client_session.role_with_space(2), Some(RoleType::User));
    assert_eq!(client_session.role_with_space(3), Some(RoleType::Guest));

    // Get All Characters
    let roles = client_session.roles();
    assert_eq!(roles.len(), 3);
}

#[test]
fn test_client_session_space_info() {
    let session = create_test_session("testuser");
    let client_session = ClientSession::new(session);

    // Initially there is no Space
    assert!(client_session.space().is_none());

    // Setting Space
    let space_info = SpaceInfo {
        space_name: "test_space".to_string(),
        space_id: 1,
        ..Default::default()
    };
    client_session.set_space(space_info.clone().into());

    // Verify Space Information
    let space = client_session.space();
    assert!(space.is_some());
    assert_eq!(space.expect("应该获取到SpaceInfo").name, "test_space");
    // space_name() 从 Session 结构体读取，set_space() 设置到 SpaceInfo 结构体
    // 两者是独立的存储，这里只验证 space() 返回正确的 SpaceInfo
}

// ==================== Comprehensive Scenario Testing ====================

#[test]
fn test_complete_permission_workflow() {
    // Creating a Rights Manager
    let pm = PermissionManager::new();
    let config = create_test_config();
    let checker = PermissionChecker::new(pm, config);
    let space_id = 1i64;

    // 1. God created Space (simulation)
    let root_session = create_client_session_with_role("root", 0, RoleType::God);
    assert!(checker.can_write_space(&root_session).is_ok());

    // 2. God creates Admin user and grants Admin role
    checker
        .permission_manager()
        .grant_role("admin1", space_id, RoleType::Admin)
        .expect("授予Admin角色应该成功");

    // 3. Admin creates Dba user and grants Dba role
    let admin_session = create_client_session_with_role("admin1", space_id, RoleType::Admin);
    assert!(checker
        .can_write_role(&admin_session, space_id, "dba1", RoleType::Dba)
        .is_ok());
    checker
        .permission_manager()
        .grant_role("dba1", space_id, RoleType::Dba)
        .expect("授予Dba角色应该成功");

    // 4. Dba creates regular user and grants User role
    let dba_session = create_client_session_with_role("dba1", space_id, RoleType::Dba);
    assert!(checker
        .can_write_role(&dba_session, space_id, "user1", RoleType::User)
        .is_ok());
    checker
        .permission_manager()
        .grant_role("user1", space_id, RoleType::User)
        .expect("授予User角色应该成功");

    // 5. Verification of individual user rights
    // Admin can manage Schema
    assert!(checker
        .permission_manager()
        .check_permission("admin1", space_id, Permission::Schema)
        .is_ok());

    // Dba can manage Schema
    assert!(checker
        .permission_manager()
        .check_permission("dba1", space_id, Permission::Schema)
        .is_ok());

    // Normal users cannot manage Schema
    assert!(checker
        .permission_manager()
        .check_permission("user1", space_id, Permission::Schema)
        .is_err());

    // 6. List all users in Space
    let users = checker.permission_manager().list_space_users(space_id);
    assert_eq!(users.len(), 3);

    // 7. Withdrawal of roles
    checker
        .permission_manager()
        .revoke_role("user1", space_id)
        .expect("撤销角色应该成功");
    assert_eq!(
        checker.permission_manager().get_role("user1", space_id),
        None
    );
}

#[test]
fn test_cross_space_permission_isolation() {
    let pm = PermissionManager::new();

    // Users have different roles in different spaces.
    pm.grant_role("user1", 1, RoleType::Admin)
        .expect("授予Admin角色应该成功");
    pm.grant_role("user1", 2, RoleType::User)
        .expect("授予User角色应该成功");

    // You have all permissions in Space 1 (Admin).
    assert!(pm.check_permission("user1", 1, Permission::Schema).is_ok());
    assert!(pm.check_permission("user1", 1, Permission::Admin).is_ok());

    // In Space 2 (User), the only permissions available are read, write, and delete.
    assert!(pm.check_permission("user1", 2, Permission::Read).is_ok());
    assert!(pm.check_permission("user1", 2, Permission::Write).is_ok());
    assert!(pm.check_permission("user1", 2, Permission::Schema).is_err());

    // In Space 3 (without any characters), there are no permissions available at all.
    assert!(pm.check_permission("user1", 3, Permission::Read).is_err());
}
