//! User Management Type Definition

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PasswordInfo {
    pub username: Option<String>,
    pub old_password: String,
    pub new_password: String,
}

/// User information - refer to nebula-graph UserItem implementation
/// Includes password hashes and resource limits
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserInfo {
    pub username: String,
    /// Password hashing (bcrypt encryption)
    pub password_hash: String,
    /// Locked or not
    pub is_locked: bool,
    /// Maximum number of queries per hour (0 means unlimited)
    pub max_queries_per_hour: i32,
    /// Maximum number of updates per hour (0 means unlimited)
    pub max_updates_per_hour: i32,
    /// Maximum number of connections per hour (0 means unlimited)
    pub max_connections_per_hour: i32,
    /// Maximum number of concurrent connections (0 means unlimited)
    pub max_user_connections: i32,
    /// Creation time
    pub created_at: i64,
    /// Last login time
    pub last_login_at: Option<i64>,
    /// Password last modified time
    pub password_changed_at: i64,
}

impl UserInfo {
    /// Create a new user (using plaintext passwords, internal autohashing)
    pub fn new(username: String, password: String) -> Result<Self, crate::core::StorageError> {
        let password_hash = bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| {
            crate::core::StorageError::db_error(format!("Password encryption failed: {}", e))
        })?;

        let now = chrono::Utc::now().timestamp_millis();

        Ok(Self {
            username,
            password_hash,
            is_locked: false,
            max_queries_per_hour: 0,
            max_updates_per_hour: 0,
            max_connections_per_hour: 0,
            max_user_connections: 0,
            created_at: now,
            last_login_at: None,
            password_changed_at: now,
        })
    }

    /// Verify Password
    pub fn verify_password(&self, password: &str) -> bool {
        bcrypt::verify(password, &self.password_hash).unwrap_or(false)
    }

    /// change your password
    pub fn change_password(
        &mut self,
        new_password: String,
    ) -> Result<(), crate::core::StorageError> {
        self.password_hash = bcrypt::hash(new_password, bcrypt::DEFAULT_COST).map_err(|e| {
            crate::core::StorageError::db_error(format!("Password encryption failed: {}", e))
        })?;
        self.password_changed_at = chrono::Utc::now().timestamp_millis();
        Ok(())
    }

    pub fn with_locked(mut self, is_locked: bool) -> Self {
        self.is_locked = is_locked;
        self
    }

    pub fn with_max_queries_per_hour(mut self, limit: i32) -> Self {
        self.max_queries_per_hour = limit;
        self
    }

    pub fn with_max_updates_per_hour(mut self, limit: i32) -> Self {
        self.max_updates_per_hour = limit;
        self
    }

    pub fn with_max_connections_per_hour(mut self, limit: i32) -> Self {
        self.max_connections_per_hour = limit;
        self
    }

    pub fn with_max_user_connections(mut self, limit: i32) -> Self {
        self.max_user_connections = limit;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserAlterInfo {
    pub username: String,
    /// New lock status
    pub is_locked: Option<bool>,
    /// New maximum number of queries per hour
    pub max_queries_per_hour: Option<i32>,
    /// New maximum number of updates per hour
    pub max_updates_per_hour: Option<i32>,
    /// New maximum number of connections per hour
    pub max_connections_per_hour: Option<i32>,
    /// New maximum number of concurrent connections
    pub max_user_connections: Option<i32>,
}

impl UserAlterInfo {
    pub fn new(username: String) -> Self {
        Self {
            username,
            is_locked: None,
            max_queries_per_hour: None,
            max_updates_per_hour: None,
            max_connections_per_hour: None,
            max_user_connections: None,
        }
    }

    pub fn with_locked(mut self, is_locked: bool) -> Self {
        self.is_locked = Some(is_locked);
        self
    }
}
