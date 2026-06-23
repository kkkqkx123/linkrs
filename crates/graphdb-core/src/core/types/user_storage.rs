//! User Storage Manager
//!
//! Manages user account creation, modification, deletion, and role authorization.
//! This storage is in-memory by default and can be persisted to a JSON snapshot.

use crate::core::types::{PasswordInfo, UserAlterInfo, UserInfo};
use crate::core::{RoleType, StorageError, StorageResult};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

const USER_STORAGE_FORMAT_VERSION: u32 = 1;
const USER_STORAGE_FILE_NAME: &str = "users.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserStorageSnapshot {
    version: u32,
    users: Vec<UserInfo>,
}

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

    fn snapshot(&self) -> UserStorageSnapshot {
        let mut users: Vec<UserInfo> = self.users.write().values().cloned().collect();
        users.sort_by(|left, right| left.username.cmp(&right.username));

        UserStorageSnapshot {
            version: USER_STORAGE_FORMAT_VERSION,
            users,
        }
    }

    /// Clear all users.
    pub fn clear(&self) {
        self.users.write().clear();
    }

    /// Persist users to a directory snapshot.
    pub fn save_to_dir<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        fs::create_dir_all(path)?;

        let snapshot = self.snapshot();
        let content = serde_json::to_string_pretty(&snapshot).map_err(|e| {
            StorageError::serialize_error(format!("Failed to serialize user storage: {}", e))
        })?;

        let file_path = path.join(USER_STORAGE_FILE_NAME);
        fs::write(&file_path, content).map_err(|e| {
            StorageError::io_error(format!(
                "Failed to write user storage file {}: {}",
                file_path.display(),
                e
            ))
        })?;
        Ok(())
    }

    /// Load users from a directory snapshot.
    pub fn load_from_dir<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        let file_path = path.join(USER_STORAGE_FILE_NAME);

        if !file_path.exists() {
            self.clear();
            return Ok(());
        }

        let content = fs::read_to_string(&file_path).map_err(|e| {
            StorageError::io_error(format!(
                "Failed to read user storage file {}: {}",
                file_path.display(),
                e
            ))
        })?;
        let snapshot: UserStorageSnapshot = serde_json::from_str(&content).map_err(|e| {
            StorageError::deserialize_error(format!("Failed to deserialize user storage: {}", e))
        })?;

        if snapshot.version != USER_STORAGE_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported user storage version: {}",
                snapshot.version
            )));
        }

        let mut users = HashMap::with_capacity(snapshot.users.len());
        for user in snapshot.users {
            if users.insert(user.username.clone(), user).is_some() {
                return Err(StorageError::deserialize_error(
                    "Duplicate user entry in user storage snapshot".to_string(),
                ));
            }
        }

        *self.users.write() = users;
        Ok(())
    }

    /// Change the user password.
    pub fn change_password(&self, info: &PasswordInfo) -> Result<bool, StorageError> {
        let mut users = self.users.write();
        let username = info
            .username
            .clone()
            .ok_or_else(|| StorageError::db_error("Username cannot be empty".to_string()))?;
        if let Some(user) = users.get_mut(&username) {
            // Verify old password first
            if !user.verify_password(&info.old_password) {
                return Err(StorageError::db_error(
                    "Old password verification failed".to_string(),
                ));
            }
            // Change to new password
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
            return Ok(true);
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
        let existed = users.remove(username).is_some();
        Ok(existed)
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
        assert!(result.unwrap());
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

    #[test]
    fn test_save_and_load_round_trip() {
        let base_dir = std::env::temp_dir()
            .join("graphdb_user_storage_test")
            .join(format!("round_trip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base_dir);
        std::fs::create_dir_all(&base_dir).expect("create temp dir should succeed");

        let storage = UserStorage::new();
        let alice = UserInfo::new("alice".to_string(), "secret".to_string())
            .expect("UserInfo::new should succeed");
        let bob = UserInfo::new("bob".to_string(), "password".to_string())
            .expect("UserInfo::new should succeed")
            .with_locked(true)
            .with_max_queries_per_hour(12);

        storage
            .create_user(&alice)
            .expect("create alice should succeed");
        storage
            .create_user(&bob)
            .expect("create bob should succeed");
        storage
            .save_to_dir(&base_dir)
            .expect("save_to_dir should succeed");

        let restored = UserStorage::new();
        restored
            .load_from_dir(&base_dir)
            .expect("load_from_dir should succeed");

        assert!(restored.user_exists("alice"));
        assert!(restored.user_exists("bob"));

        let loaded_alice = restored
            .get_user("alice")
            .expect("alice should exist after load");
        assert!(loaded_alice.verify_password("secret"));

        let loaded_bob = restored
            .get_user("bob")
            .expect("bob should exist after load");
        assert!(loaded_bob.is_locked);
        assert_eq!(loaded_bob.max_queries_per_hour, 12);

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
