//! User Storage Manager
//!
//! Manages user account creation, modification, deletion, and role authorization.
//! This is a pure in-memory storage for user metadata, separate from graph data storage.

use crate::core::types::{PasswordInfo, UserAlterInfo, UserInfo};
use crate::core::{RoleType, StorageError};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Manages user accounts and role assignments in memory.
#[derive(Clone)]
pub struct UserStorage {
    users: Arc<RwLock<HashMap<String, UserInfo>>>,
}

impl std::fmt::Debug for UserStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserStorage")
            .field("user_count", &self.users.write().len())
            .finish()
    }
}

impl Default for UserStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl UserStorage {
    /// Create a new user storage instance.
    pub fn new() -> Self {
        Self {
            users: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Change the user password.
    pub fn change_password(&self, info: &PasswordInfo) -> Result<bool, StorageError> {
        let mut users = self.users.write();
        let username = info
            .username
            .clone()
            .ok_or_else(|| StorageError::db_error("Username cannot be empty".to_string()))?;
        if let Some(user) = users.get_mut(&username) {
            user.change_password(info.new_password.clone())?;
            Ok(true)
        } else {
            Err(StorageError::db_error(format!(
                "User {} does not exist",
                username
            )))
        }
    }

    /// Create a new user.
    pub fn create_user(&self, info: &UserInfo) -> Result<bool, StorageError> {
        let mut users = self.users.write();
        if users.contains_key(&info.username) {
            return Err(StorageError::db_error(format!(
                "User {} already exists",
                info.username
            )));
        }
        users.insert(info.username.clone(), info.clone());
        Ok(true)
    }

    /// Modify user information.
    pub fn alter_user(&self, info: &UserAlterInfo) -> Result<bool, StorageError> {
        let mut users = self.users.write();
        if let Some(user) = users.get_mut(&info.username) {
            if let Some(is_locked) = info.is_locked {
                user.is_locked = is_locked;
            }
            if let Some(limit) = info.max_queries_per_hour {
                user.max_queries_per_hour = limit;
            }
            if let Some(limit) = info.max_updates_per_hour {
                user.max_updates_per_hour = limit;
            }
            if let Some(limit) = info.max_connections_per_hour {
                user.max_connections_per_hour = limit;
            }
            if let Some(limit) = info.max_user_connections {
                user.max_user_connections = limit;
            }
            Ok(true)
        } else {
            Err(StorageError::db_error(format!(
                "User {} does not exist",
                info.username
            )))
        }
    }

    /// Delete the user.
    pub fn drop_user(&self, username: &str) -> Result<bool, StorageError> {
        let mut users = self.users.write();
        users.remove(username);
        Ok(true)
    }

    /// Get user information.
    pub fn get_user(&self, username: &str) -> Option<UserInfo> {
        self.users.write().get(username).cloned()
    }

    /// Check whether the user exists.
    pub fn user_exists(&self, username: &str) -> bool {
        self.users.write().contains_key(username)
    }

    /// Grant roles to user (only verifies user existence; actual authorization is handled by PermissionManager).
    pub fn grant_role(
        &self,
        username: &str,
        _space_id: u64,
        _role: RoleType,
    ) -> Result<bool, StorageError> {
        let users = self.users.write();
        if users.contains_key(username) {
            Ok(true)
        } else {
            Err(StorageError::db_error(format!(
                "User {} not found",
                username
            )))
        }
    }

    /// Revoke roles from user (only verifies user existence; actual revocation is handled by PermissionManager).
    pub fn revoke_role(&self, username: &str, _space_id: u64) -> Result<bool, StorageError> {
        let users = self.users.write();
        if users.contains_key(username) {
            Ok(true)
        } else {
            Err(StorageError::db_error(format!(
                "User {} not found",
                username
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::RoleType;

    #[test]
    fn test_create_user() {
        let storage = UserStorage::new();
        let user = UserInfo {
            username: "test_user".to_string(),
            password_hash: "hash".to_string(),
            is_locked: false,
            max_queries_per_hour: 0,
            max_updates_per_hour: 0,
            max_connections_per_hour: 0,
            max_user_connections: 0,
            created_at: 0,
            last_login_at: None,
            password_changed_at: 0,
        };

        assert!(storage
            .create_user(&user)
            .expect("create_user should succeed"));
        assert!(storage.user_exists("test_user"));
    }

    #[test]
    fn test_create_duplicate_user() {
        let storage = UserStorage::new();
        let user = UserInfo {
            username: "test_user".to_string(),
            password_hash: "hash".to_string(),
            is_locked: false,
            max_queries_per_hour: 0,
            max_updates_per_hour: 0,
            max_connections_per_hour: 0,
            max_user_connections: 0,
            created_at: 0,
            last_login_at: None,
            password_changed_at: 0,
        };

        storage
            .create_user(&user)
            .expect("create_user should succeed");
        let result = storage.create_user(&user);
        assert!(result.is_err());
    }

    #[test]
    fn test_drop_user() {
        let storage = UserStorage::new();
        let user = UserInfo {
            username: "test_user".to_string(),
            password_hash: "hash".to_string(),
            is_locked: false,
            max_queries_per_hour: 0,
            max_updates_per_hour: 0,
            max_connections_per_hour: 0,
            max_user_connections: 0,
            created_at: 0,
            last_login_at: None,
            password_changed_at: 0,
        };

        storage
            .create_user(&user)
            .expect("create_user should succeed");
        assert!(storage
            .drop_user("test_user")
            .expect("drop_user should succeed"));
        assert!(!storage.user_exists("test_user"));
    }

    #[test]
    fn test_alter_user() {
        let storage = UserStorage::new();
        let user = UserInfo {
            username: "test_user".to_string(),
            password_hash: "hash".to_string(),
            is_locked: false,
            max_queries_per_hour: 0,
            max_updates_per_hour: 0,
            max_connections_per_hour: 0,
            max_user_connections: 0,
            created_at: 0,
            last_login_at: None,
            password_changed_at: 0,
        };

        storage
            .create_user(&user)
            .expect("create_user should succeed");

        let alter_info = UserAlterInfo {
            username: "test_user".to_string(),
            is_locked: Some(true),
            max_queries_per_hour: Some(100),
            max_updates_per_hour: None,
            max_connections_per_hour: None,
            max_user_connections: None,
        };

        assert!(storage
            .alter_user(&alter_info)
            .expect("alter_user should succeed"));

        let updated_user = storage
            .get_user("test_user")
            .expect("get_user should succeed");
        assert!(updated_user.is_locked);
        assert_eq!(updated_user.max_queries_per_hour, 100);
    }

    #[test]
    fn test_grant_role_user_not_found() {
        let storage = UserStorage::new();
        let result = storage.grant_role("nonexistent", 1, RoleType::Admin);
        assert!(result.is_err());
    }

    #[test]
    fn test_revoke_role_user_not_found() {
        let storage = UserStorage::new();
        let result = storage.revoke_role("nonexistent", 1);
        assert!(result.is_err());
    }
}
