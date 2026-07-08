use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

use super::PermissionResult;

// Re-export the permission types from the core layer.
pub use crate::core::{Permission, RoleType};

/// The Space ID identifier for the God character (a global character, not bound to a specific Space)
pub const GOD_SPACE_ID: i64 = -1;

/// Permission Manager – Data Layer
///
/// Responsibilities:
/// 1. Manage Role mapping（username -> {space_id -> role}）
/// 2. Manage space mapping\（space_id -> {username -> [permissions]}）
/// 3. Provide basic character query and permission checking functions.
///
/// This layer does not involve any business logic decisions (such as the priority of the God character); it only provides basic data operations.
///
/// Performance optimization:
/// Use DashMap to achieve true concurrent access without the need for explicit locking.
/// Outstanding performance in scenarios where more reading and less writing are required.
pub struct PermissionManager {
    /// User role mapping：username -> {space_id -> role}
    /// The “God” role uses a special space_id: -1 to indicate that it is a global role, which is not associated with a specific Space.
    /// Using DashMap, concurrent read and write operations are supported.
    user_roles: Arc<DashMap<String, HashMap<i64, RoleType>>>,
    /// space permission mapping：space_id -> {username -> [permissions]}
    /// Used for fine-grained permission control
    /// Use DashMap to support async read/write
    space_permissions: Arc<DashMap<i64, HashMap<String, Vec<Permission>>>>,
}

impl PermissionManager {
    /// Create a new permission manager.
    pub fn new() -> Self {
        let user_roles = DashMap::new();
        let mut root_roles = HashMap::new();
        // The root user acts as the "God" character (the global super administrator).
        // Using GOD_SPACE_ID(-1) to represent global roles, not bound to specific Space
        root_roles.insert(GOD_SPACE_ID, RoleType::God);
        user_roles.insert("root".to_string(), root_roles);

        Self {
            user_roles: Arc::new(user_roles),
            space_permissions: Arc::new(DashMap::new()),
        }
    }

    // ==================== Role Management (Basic CRUD) ====================

    /// Granting roles
    ///
    /// Using the entry API of DashMap, there is no need to explicitly acquire locks.
    pub fn grant_role(
        &self,
        username: &str,
        space_id: i64,
        role: RoleType,
    ) -> PermissionResult<()> {
        self.user_roles
            .entry(username.to_string())
            .or_default()
            .insert(space_id, role);
        Ok(())
    }

    /// Revoke the role
    ///
    /// Using DashMap, there is no need to explicitly apply locks.
    pub fn revoke_role(&self, username: &str, space_id: i64) -> PermissionResult<()> {
        if let Some(mut roles) = self.user_roles.get_mut(username) {
            roles.remove(&space_id);
        }
        Ok(())
    }

    /// Obtain the user's role in the specified space.
    ///
    /// DashMap supports true concurrent reading.
    pub fn get_role(&self, username: &str, space_id: i64) -> Option<RoleType> {
        self.user_roles
            .get(username)
            .and_then(|roles| roles.get(&space_id).copied())
    }

    /// Obtain all the user's roles
    pub fn get_user_roles(&self, username: &str) -> HashMap<i64, RoleType> {
        self.user_roles
            .get(username)
            .map(|roles| roles.clone())
            .unwrap_or_default()
    }

    /// List all of the user’s roles.
    /// Return a Vec<(space_id, role)> containing the user's roles in all Spaces.
    pub fn list_user_roles(&self, username: &str) -> Vec<(i64, RoleType)> {
        self.user_roles
            .get(username)
            .map(|roles| {
                roles
                    .iter()
                    .map(|(&space_id, &role)| (space_id, role))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// List all users in Space and their respective roles.
    /// Return a Vec<(username, role)> containing all users in the specified Space.
    ///
    /// DashMap supports true concurrent reading; locking is not required during iteration.
    pub fn list_space_users(&self, space_id: i64) -> Vec<(String, RoleType)> {
        self.user_roles
            .iter()
            .filter_map(|entry| {
                let username = entry.key();
                let roles = entry.value();
                roles.get(&space_id).map(|&role| (username.clone(), role))
            })
            .collect()
    }

    // ==================== Role Query (Basic Query) ====================

    /// Check whether the user is the God character.
    pub fn is_god(&self, username: &str) -> bool {
        self.user_roles
            .get(username)
            .map(|roles| roles.values().any(|&role| role == RoleType::God))
            .unwrap_or(false)
    }

    /// Check whether the user is an administrator (with the God or Admin role).
    pub fn is_admin(&self, username: &str) -> bool {
        self.user_roles
            .get(username)
            .map(|roles| {
                roles
                    .values()
                    .any(|&role| matches!(role, RoleType::God | RoleType::Admin))
            })
            .unwrap_or(false)
    }

    /// Check whether the user has the specified role in the designated space.
    pub fn has_role(&self, username: &str, space_id: i64, role: RoleType) -> bool {
        self.get_role(username, space_id)
            .map(|r| r == role)
            .unwrap_or(false)
    }

    // ==================== Permission Check (Basic Verification) ====================

    /// Basic permission check
    /// Check whether the user has the specified permissions in the designated space.
    pub fn check_permission(
        &self,
        username: &str,
        space_id: i64,
        permission: Permission,
    ) -> PermissionResult<()> {
        use crate::api::server::permission::PermissionError;

        let role = self
            .get_role(username, space_id)
            .or_else(|| self.get_role(username, GOD_SPACE_ID))
            .ok_or_else(|| PermissionError::no_role_in_space(username.to_string(), space_id))?;

        if role.has_permission(permission) {
            Ok(())
        } else {
            Err(PermissionError::permission_denied(
                format!("{:?}", permission),
                username.to_string(),
            ))
        }
    }

    /// Check whether the user can be granted the role.
    pub fn can_grant_role(&self, granter: &str, space_id: i64, target_role: RoleType) -> bool {
        self.user_roles
            .get(granter)
            .and_then(|roles| roles.get(&space_id).copied())
            .map(|role| role.can_grant(target_role))
            .unwrap_or(false)
    }

    /// Check whether the user can revoke the assigned role.
    pub fn can_revoke_role(&self, revoker: &str, space_id: i64, target_role: RoleType) -> bool {
        self.can_grant_role(revoker, space_id, target_role)
    }

    // ==================== Space Permission Management (Fine-Grained Permissions) ====================

    /// Granting specific permissions to users in the space
    pub fn grant_permission(
        &self,
        username: &str,
        space_id: i64,
        permission: Permission,
    ) -> PermissionResult<()> {
        let mut space_map = self.space_permissions.entry(space_id).or_default();

        let user_permissions = space_map.entry(username.to_string()).or_default();

        if !user_permissions.contains(&permission) {
            user_permissions.push(permission);
        }
        Ok(())
    }

    /// Revoke a user's specific permissions in the space
    pub fn revoke_permission(
        &self,
        username: &str,
        space_id: i64,
        permission: Permission,
    ) -> PermissionResult<()> {
        if let Some(mut space_map) = self.space_permissions.get_mut(&space_id) {
            if let Some(user_permissions) = space_map.get_mut(username) {
                user_permissions.retain(|&p| p != permission);
            }
        }
        Ok(())
    }

    /// Obtain a list of specific permissions that a user has in the space.
    pub fn get_permissions(&self, username: &str, space_id: i64) -> Vec<Permission> {
        self.space_permissions
            .get(&space_id)
            .and_then(|space_map| space_map.get(username).cloned())
            .unwrap_or_default()
    }

    /// Checking whether a user has specific privileges in space (fine-grained checking)
    pub fn has_permission(&self, username: &str, space_id: i64, permission: Permission) -> bool {
        self.get_permissions(username, space_id)
            .contains(&permission)
    }
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_and_get_role() {
        let pm = PermissionManager::new();

        pm.grant_role("user1", 1, RoleType::Admin)
            .expect("Failed to grant role");

        assert_eq!(pm.get_role("user1", 1), Some(RoleType::Admin));
        assert_eq!(pm.get_role("user1", 2), None);
        assert_eq!(pm.get_role("nonexistent", 1), None);
    }

    #[test]
    fn test_is_god() {
        let pm = PermissionManager::new();

        // The default is God.
        assert!(pm.is_god("root"));

        let _ = pm.grant_role("user1", 1, RoleType::Admin);
        assert!(!pm.is_god("user1"));

        let _ = pm.grant_role("user2", GOD_SPACE_ID, RoleType::God);
        assert!(pm.is_god("user2"));
    }

    #[test]
    fn test_check_permission() {
        let pm = PermissionManager::new();

        pm.grant_role("user1", 1, RoleType::User)
            .expect("Failed to grant role");
        pm.grant_role("guest1", 1, RoleType::Guest)
            .expect("Failed to grant role");

        // The User role has Read and Write permissions
        assert!(pm.check_permission("user1", 1, Permission::Read).is_ok());
        assert!(pm.check_permission("user1", 1, Permission::Write).is_ok());
        // The Guest role has only Read privileges
        assert!(pm.check_permission("guest1", 1, Permission::Read).is_ok());
        assert!(pm.check_permission("guest1", 1, Permission::Write).is_err());
        // unauthorized user
        assert!(pm
            .check_permission("nonexistent", 1, Permission::Read)
            .is_err());
    }

    #[test]
    fn test_can_grant_role() {
        let pm = PermissionManager::new();

        pm.grant_role("admin", 1, RoleType::Admin)
            .expect("Failed to grant role");
        pm.grant_role("user", 1, RoleType::User)
            .expect("Failed to grant role");

        // Admin can grant the User and Guest roles
        assert!(pm.can_grant_role("admin", 1, RoleType::User));
        assert!(pm.can_grant_role("admin", 1, RoleType::Guest));
        // Admin cannot grant the Admin or God role.
        assert!(!pm.can_grant_role("admin", 1, RoleType::Admin));
        assert!(!pm.can_grant_role("admin", 1, RoleType::God));

        // User cannot be granted any role
        assert!(!pm.can_grant_role("user", 1, RoleType::Guest));
    }

    #[test]
    fn test_god_role_global_permission() {
        let pm = PermissionManager::new();

        // God roles have permissions in any space
        assert!(pm.check_permission("root", 999, Permission::Write).is_ok());
        assert!(pm.check_permission("root", 999, Permission::Read).is_ok());
    }

    #[test]
    fn test_list_user_roles() {
        let pm = PermissionManager::new();

        // Granting different roles to users in different Space
        pm.grant_role("multi_user", 1, RoleType::Admin)
            .expect("Failed to grant role");
        pm.grant_role("multi_user", 2, RoleType::User)
            .expect("Failed to grant role");
        pm.grant_role("multi_user", 3, RoleType::Guest)
            .expect("Failed to grant role");

        // List all roles of a user
        let roles = pm.list_user_roles("multi_user");
        assert_eq!(roles.len(), 3);

        // Verify that the correct roles are included
        let role_map: HashMap<i64, RoleType> = roles.into_iter().collect();
        assert_eq!(role_map.get(&1), Some(&RoleType::Admin));
        assert_eq!(role_map.get(&2), Some(&RoleType::User));
        assert_eq!(role_map.get(&3), Some(&RoleType::Guest));

        // Returns an empty list for non-existent users
        let empty_roles = pm.list_user_roles("nonexistent");
        assert!(empty_roles.is_empty());
    }

    #[test]
    fn test_list_space_users() {
        let pm = PermissionManager::new();
        let space_id = 1i64;

        // Granting roles to multiple users
        pm.grant_role("user1", space_id, RoleType::User)
            .expect("Failed to grant role");
        pm.grant_role("user2", space_id, RoleType::Admin)
            .expect("Failed to grant role");
        pm.grant_role("user3", space_id, RoleType::Guest)
            .expect("Failed to grant role");

        // List all users in Space
        let users = pm.list_space_users(space_id);
        assert_eq!(users.len(), 3);

        // Verify that the correct users and roles are included
        let user_map: HashMap<String, RoleType> = users.into_iter().collect();
        assert_eq!(user_map.get("user1"), Some(&RoleType::User));
        assert_eq!(user_map.get("user2"), Some(&RoleType::Admin));
        assert_eq!(user_map.get("user3"), Some(&RoleType::Guest));

        // Empty Space returns an empty list
        let empty_users = pm.list_space_users(999);
        assert!(empty_users.is_empty());
    }
}
