use super::PermissionResult;
use crate::api::server::permission::{PermissionManager, GOD_SPACE_ID};
use crate::api::server::session::ClientSession;
use crate::config::AuthConfig;
use crate::core::{Permission, RoleType};

/// Operation type – corresponds to different permission checks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    // Space operations
    ReadSpace,  // USE, DESCRIBE SPACE
    WriteSpace, // CREATE SPACE, DROP SPACE, CLEAR SPACE

    // Schema operations
    ReadSchema,  // DESCRIBE TAG, DESCRIBE EDGE
    WriteSchema, // CREATE TAG, ALTER TAG, CREATE EDGE, DROP TAG

    // Data manipulation
    ReadData,  // GO, MATCH, FETCH, LOOKUP
    WriteData, // INSERT, UPDATE, DELETE

    // User operations
    ReadUser,  // DESCRIBE USER
    WriteUser, // CREATE USER, DROP USER, ALTER USER

    // Character actions
    WriteRole, // GRANT, REVOKE

    // Special operations
    Show,           // “SHOW SPACES”, “SHOW USERS” etc.
    ChangePassword, // CHANGE PASSWORD
}

/// Permission Checker – Business Layer
///
/// Responsibilities:
/// Provide a unified entry point for permission checks.
/// 2. Implement business logic decisions (such as giving priority to the ‘God’ role, imposing restrictions on the ‘Guest’ role, etc.).
/// 3. Managing authorization settings (whether to enable authorization)
/// 4. The basic operations of combining the PermissionManager are used to perform complex permission checks.
///
/// Design principles:
/// All business logic is implemented at this layer.
/// Do not directly manipulate the permission data; instead, access it through the PermissionManager.
pub struct PermissionChecker {
    permission_manager: PermissionManager,
    auth_config: AuthConfig,
}

impl PermissionChecker {
    /// Create a new permission checker.
    pub fn new(permission_manager: PermissionManager, auth_config: AuthConfig) -> Self {
        Self {
            permission_manager,
            auth_config,
        }
    }

    /// Check whether authorization is enabled.
    fn is_authorization_enabled(&self) -> bool {
        self.auth_config.enable_authorize
    }

    // ==================== Unified permission checking entry ====================

    /// Unified permission checking entry point
    ///
    /// Perform the corresponding permission checks based on the type of operation, taking into account the business logic.
    /// The God character possesses all permissions.
    /// Writing is restricted for the Guest role.
    /// Users can only change their own passwords.
    pub fn check_permission(
        &self,
        session: &ClientSession,
        operation: OperationType,
        target_space: Option<i64>,
        target_user: Option<&str>,
        target_role: Option<RoleType>,
    ) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        // If authorization is not enabled, a successful response is returned directly.
        if !self.is_authorization_enabled() {
            return Ok(());
        }

        let username = session.user();

        // The God character has all permissions (except that modifying the password requires a special verification process).
        if self.permission_manager.is_god(&username) {
            match operation {
                // Even God could only change their own password or the passwords of users who have been explicitly authorized to do so.
                OperationType::ChangePassword => {
                    return self.check_change_password(&username, target_user, session);
                }
                _ => return Ok(()),
            }
        }

        match operation {
            // Space read operation: USE, DESCRIBE SPACE
            OperationType::ReadSpace => self.check_read_space(&username, target_space),

            // Space-related operations: CREATE SPACE, DROP SPACE, etc.
            // Only the God character can perform this action.
            OperationType::WriteSpace => Err(PermissionError::only_god_can_manage_spaces()),

            // Schema reading operation
            OperationType::ReadSchema => self.check_read_schema(&username, target_space),

            // Schema writing operation
            OperationType::WriteSchema => self.check_write_schema(&username, target_space),

            // Data reading operation
            OperationType::ReadData => self.check_read_data(&username, target_space),

            // Data writing operation
            OperationType::WriteData => self.check_write_data(session, &username, target_space),

            // User reading operation
            OperationType::ReadUser => self.check_read_user(&username, target_user, session),

            // User-written operations
            OperationType::WriteUser => Err(PermissionError::only_god_can_manage_users()),

            // Role management operations: GRANT, REVOKE
            OperationType::WriteRole => {
                self.check_write_role(&username, target_space, target_user, target_role)
            }

            // Display the operation.
            OperationType::Show => {
                // The SHOW operation usually allows all authenticated users to access the relevant information.
                Ok(())
            }

            // Password change operation
            OperationType::ChangePassword => {
                self.check_change_password(&username, target_user, session)
            }
        }
    }

    // ==================== Methods for Checking Specific Business Logic ====================

    /// Check Space read permissions
    fn check_read_space(&self, username: &str, target_space: Option<i64>) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        let space_id = target_space.ok_or(PermissionError::space_id_required())?;

        // Basic checks using PermissionManager
        self.permission_manager
            .check_permission(username, space_id, Permission::Read)
    }

    /// Checking Schema Read Permissions
    fn check_read_schema(&self, username: &str, target_space: Option<i64>) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        let space_id = target_space.ok_or(PermissionError::schema_space_id_required())?;

        self.permission_manager
            .check_permission(username, space_id, Permission::Read)
    }

    /// Checking Schema Write Permissions
    /// Business logic: Only God and Admin have the permission to write to the Schema.
    fn check_write_schema(
        &self,
        username: &str,
        target_space: Option<i64>,
    ) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        let space_id = target_space.ok_or(PermissionError::schema_write_space_id_required())?;

        // Check whether the user is an administrator.
        if !self.permission_manager.is_admin(username) {
            return Err(PermissionError::schema_write_permission_denied(
                space_id,
                username.to_string(),
            ));
        }

        // The administrator needs the “Write” permission.
        self.permission_manager
            .check_permission(username, space_id, Permission::Write)
    }

    /// Checking data read permissions
    fn check_read_data(&self, username: &str, target_space: Option<i64>) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        let space_id = target_space.ok_or(PermissionError::data_read_space_id_required())?;

        self.permission_manager
            .check_permission(username, space_id, Permission::Read)
    }

    /// Checking data write permissions
    /// Business Logic: Guest Role Cannot Write Data
    fn check_write_data(
        &self,
        session: &ClientSession,
        username: &str,
        target_space: Option<i64>,
    ) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        let space_id = target_space.ok_or(PermissionError::data_write_space_id_required())?;

        // Guest roles cannot write data
        if let Some(role) = session.role_with_space(space_id) {
            if role == RoleType::Guest {
                return Err(PermissionError::guest_cannot_write_data());
            }
        }

        self.permission_manager
            .check_permission(username, space_id, Permission::Write)
    }

    /// Checking user read permissions
    /// Business logic: users can read their own information
    fn check_read_user(
        &self,
        username: &str,
        target_user: Option<&str>,
        _session: &ClientSession,
    ) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        // Users can read their own information
        if let Some(target) = target_user {
            if username == target {
                return Ok(());
            }
        }

        // Admin can read information about other users
        if self.permission_manager.is_admin(username) {
            return Ok(());
        }

        Err(PermissionError::cannot_read_user_info())
    }

    /// Checking role write permissions
    /// Business Logic: Admin or Dba role is required, and the target role cannot be higher than the operator, and you cannot modify your own role.
    fn check_write_role(
        &self,
        username: &str,
        target_space: Option<i64>,
        target_user: Option<&str>,
        target_role: Option<RoleType>,
    ) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        let space_id = target_space.ok_or(PermissionError::role_operation_space_id_required())?;
        let role = target_role.ok_or(PermissionError::role_operation_target_role_required())?;

        // Get the operator's role in the space
        let operator_role = self
            .permission_manager
            .get_role(username, space_id)
            .or_else(|| self.permission_manager.get_role(username, GOD_SPACE_ID));

        // Check if the operator has a rights management role (Admin, Dba or God)
        let can_manage = matches!(
            operator_role,
            Some(RoleType::God) | Some(RoleType::Admin) | Some(RoleType::Dba)
        );

        if !can_manage {
            return Err(PermissionError::only_admin_or_god_can_manage_roles());
        }

        // Can't modify your role
        if let Some(target) = target_user {
            if username == target {
                return Err(PermissionError::cannot_modify_own_role());
            }
        }

        // Check if the target role can be granted
        if let Some(op_role) = operator_role {
            if !op_role.can_grant(role) {
                return Err(PermissionError::cannot_grant_role(format!("{:?}", role)));
            }
        } else {
            return Err(PermissionError::only_admin_or_god_can_manage_roles());
        }

        Ok(())
    }

    /// Check the permission to change the password
    /// Business Logic: Users can change their own passwords, God can change any users' passwords
    fn check_change_password(
        &self,
        username: &str,
        target_user: Option<&str>,
        session: &ClientSession,
    ) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        let target = target_user.ok_or(PermissionError::change_password_target_user_required())?;

        // Users can change their passwords
        if username == target {
            return Ok(());
        }

        // God can change any user's password
        if session.is_god() {
            return Ok(());
        }

        Err(PermissionError::can_only_change_own_password())
    }

    // ==================== Convenience methods (for external calls) ====================

    /// Check the read permissions for Space.
    pub fn can_read_space(&self, session: &ClientSession, space_id: i64) -> PermissionResult<()> {
        self.check_permission(
            session,
            OperationType::ReadSpace,
            Some(space_id),
            None,
            None,
        )
    }

    /// Check Space write permissions
    pub fn can_write_space(&self, session: &ClientSession) -> PermissionResult<()> {
        self.check_permission(session, OperationType::WriteSpace, None, None, None)
    }

    /// Check Schema reading permissions
    pub fn can_read_schema(&self, session: &ClientSession, space_id: i64) -> PermissionResult<()> {
        self.check_permission(
            session,
            OperationType::ReadSchema,
            Some(space_id),
            None,
            None,
        )
    }

    /// Check the permissions for writing to the Schema.
    pub fn can_write_schema(&self, session: &ClientSession, space_id: i64) -> PermissionResult<()> {
        self.check_permission(
            session,
            OperationType::WriteSchema,
            Some(space_id),
            None,
            None,
        )
    }

    /// Check data reading permissions.
    pub fn can_read_data(&self, session: &ClientSession, space_id: i64) -> PermissionResult<()> {
        self.check_permission(session, OperationType::ReadData, Some(space_id), None, None)
    }

    /// Check data write permissions.
    pub fn can_write_data(&self, session: &ClientSession, space_id: i64) -> PermissionResult<()> {
        self.check_permission(
            session,
            OperationType::WriteData,
            Some(space_id),
            None,
            None,
        )
    }

    /// Check user read permissions.
    pub fn can_read_user(
        &self,
        session: &ClientSession,
        target_user: &str,
    ) -> PermissionResult<()> {
        self.check_permission(
            session,
            OperationType::ReadUser,
            None,
            Some(target_user),
            None,
        )
    }

    /// Checking user write permissions
    pub fn can_write_user(&self, session: &ClientSession) -> PermissionResult<()> {
        self.check_permission(session, OperationType::WriteUser, None, None, None)
    }

    /// Check role write permissions.
    pub fn can_write_role(
        &self,
        session: &ClientSession,
        space_id: i64,
        target_user: &str,
        target_role: RoleType,
    ) -> PermissionResult<()> {
        self.check_permission(
            session,
            OperationType::WriteRole,
            Some(space_id),
            Some(target_user),
            Some(target_role),
        )
    }

    /// Check the permission to modify passwords.
    pub fn can_change_password(
        &self,
        session: &ClientSession,
        target_user: &str,
    ) -> PermissionResult<()> {
        self.check_permission(
            session,
            OperationType::ChangePassword,
            None,
            Some(target_user),
            None,
        )
    }

    /// Check Show operating privileges
    pub fn can_show(&self, session: &ClientSession) -> PermissionResult<()> {
        self.check_permission(session, OperationType::Show, None, None, None)
    }

    // ==================== Getting internal components (for advanced scenarios) ====================

    /// Getting a reference to the rights manager
    pub fn permission_manager(&self) -> &PermissionManager {
        &self.permission_manager
    }

    /// Get Configuration
    pub fn auth_config(&self) -> &AuthConfig {
        &self.auth_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::server::permission::GOD_SPACE_ID;
    use crate::api::server::session::{ClientSession, Session};
    use crate::config::AuthConfig;
    use std::sync::Arc;

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

    fn create_test_checker() -> PermissionChecker {
        let pm = PermissionManager::new();

        // Assigning roles to test users
        pm.grant_role("user1", 1, RoleType::User)
            .expect("Failed to grant role");
        pm.grant_role("admin1", 1, RoleType::Admin)
            .expect("Failed to grant role");
        pm.grant_role("guest1", 1, RoleType::Guest)
            .expect("Failed to grant role");

        let config = create_test_config();
        PermissionChecker::new(pm, config)
    }

    fn create_test_session(username: &str, is_god: bool) -> Arc<ClientSession> {
        let session = Session {
            session_id: 1,
            user_name: username.to_string(),
            space_name: None,
            graph_addr: Some("127.0.0.1:1234".to_string()),
            timezone: None,
        };
        let client_session = ClientSession::new(session);
        if is_god {
            client_session.set_role(GOD_SPACE_ID, RoleType::God);
        }
        client_session
    }

    fn create_user_session(username: &str, role: RoleType, space_id: i64) -> Arc<ClientSession> {
        let session = Session {
            session_id: 1,
            user_name: username.to_string(),
            space_name: Some("test_space".to_string()),
            graph_addr: Some("127.0.0.1:1234".to_string()),
            timezone: None,
        };
        let client_session = ClientSession::new(session);
        client_session.set_role(space_id, role);
        client_session
    }

    #[test]
    fn test_disabled_authorization() {
        let mut config = create_test_config();
        config.enable_authorize = false;

        let pm = PermissionManager::new();
        let checker = PermissionChecker::new(pm, config);
        let session = create_test_session("any_user", false);

        // When disabling authorization, any operation should be performed through the
        assert!(checker.can_write_space(&session).is_ok());
        assert!(checker.can_write_schema(&session, 1).is_ok());
        assert!(checker.can_write_data(&session, 1).is_ok());
    }

    #[test]
    fn test_god_role_has_all_permissions() {
        let checker = create_test_checker();
        let god_session = create_test_session("root", true);

        // God can do anything.
        assert!(checker.can_write_space(&god_session).is_ok());
        assert!(checker.can_write_schema(&god_session, 1).is_ok());
        assert!(checker.can_write_data(&god_session, 1).is_ok());
        assert!(checker.can_write_user(&god_session).is_ok());
        assert!(checker
            .can_write_role(&god_session, 1, "admin", RoleType::Admin)
            .is_ok());
    }

    #[test]
    fn test_user_cannot_write_space() {
        let checker = create_test_checker();
        let user_session = create_user_session("user1", RoleType::User, 1);

        // Ordinary users cannot create/delete spaces
        assert!(checker.can_write_space(&user_session).is_err());
    }

    #[test]
    fn test_user_cannot_write_schema() {
        let checker = create_test_checker();
        let user_session = create_user_session("user1", RoleType::User, 1);

        // Normal users cannot modify Schema
        assert!(checker.can_write_schema(&user_session, 1).is_err());
    }

    #[test]
    fn test_admin_can_write_schema() {
        let checker = create_test_checker();
        let admin_session = create_user_session("admin1", RoleType::Admin, 1);

        // Admin can modify the Schema
        assert!(checker.can_write_schema(&admin_session, 1).is_ok());
    }

    #[test]
    fn test_guest_cannot_write_data() {
        let checker = create_test_checker();
        let guest_session = create_user_session("guest1", RoleType::Guest, 1);

        // Guest cannot write data.
        assert!(checker.can_write_data(&guest_session, 1).is_err());
        // Guest can read the data.
        assert!(checker.can_read_data(&guest_session, 1).is_ok());
    }

    #[test]
    fn test_user_can_read_own_info() {
        let checker = create_test_checker();
        let user_session = create_user_session("user1", RoleType::User, 1);

        // Users can read their own information
        assert!(checker.can_read_user(&user_session, "user1").is_ok());
        // Users can't read other users' information
        assert!(checker.can_read_user(&user_session, "user2").is_err());
    }

    #[test]
    fn test_change_password() {
        let checker = create_test_checker();
        let user_session = create_user_session("user1", RoleType::User, 1);

        // Users can change their passwords
        assert!(checker.can_change_password(&user_session, "user1").is_ok());
        // Users cannot change other users' passwords
        assert!(checker.can_change_password(&user_session, "user2").is_err());
    }

    #[test]
    fn test_admin_can_grant_lower_roles() {
        let checker = create_test_checker();
        let admin_session = create_user_session("admin1", RoleType::Admin, 1);

        // Admin can grant the User and Guest roles
        assert!(checker
            .can_write_role(&admin_session, 1, "user1", RoleType::User)
            .is_ok());
        assert!(checker
            .can_write_role(&admin_session, 1, "guest1", RoleType::Guest)
            .is_ok());
        // Admin cannot grant the Admin or God role.
        assert!(checker
            .can_write_role(&admin_session, 1, "admin2", RoleType::Admin)
            .is_err());
    }
}
