//! User Management Type Definition

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Default bcrypt cost factor for password hashing
const DEFAULT_BCRYPT_COST: u32 = bcrypt::DEFAULT_COST;
/// Minimum supported bcrypt cost factor
const MIN_BCRYPT_COST: u32 = 4;
/// Maximum supported bcrypt cost factor
const MAX_BCRYPT_COST: u32 = 12;

/// Process-wide bcrypt cost factor, fed by the server configuration.
/// The environment variable is only a fallback while no explicit value
/// has been installed, so runtime config updates take effect.
static BCRYPT_COST: AtomicU32 = AtomicU32::new(DEFAULT_BCRYPT_COST);
static BCRYPT_COST_SET: AtomicBool = AtomicBool::new(false);

/// Resolve the bcrypt cost factor from the `GRAPHDBC_BCRYPT_COST` environment
/// variable, falling back to the default cost when absent or invalid.
fn resolve_bcrypt_cost() -> u32 {
    std::env::var("GRAPHDBC_BCRYPT_COST")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|cost| (MIN_BCRYPT_COST..=MAX_BCRYPT_COST).contains(cost))
        .unwrap_or(DEFAULT_BCRYPT_COST)
}

/// Effective bcrypt cost factor used for password hashing
fn bcrypt_cost() -> u32 {
    if BCRYPT_COST_SET.load(Ordering::Relaxed) {
        BCRYPT_COST.load(Ordering::Relaxed)
    } else {
        resolve_bcrypt_cost()
    }
}

/// Override the bcrypt cost factor for password hashing.
///
/// Takes effect immediately for subsequent hashes. The cost is clamped to
/// the supported range [4, 12].
pub fn set_bcrypt_cost(cost: u32) {
    BCRYPT_COST.store(
        cost.clamp(MIN_BCRYPT_COST, MAX_BCRYPT_COST),
        Ordering::Relaxed,
    );
    BCRYPT_COST_SET.store(true, Ordering::Relaxed);
}

/// Verify a plaintext password against a stored bcrypt hash.
pub fn verify_password_hash(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PasswordInfo {
    pub username: Option<String>,
    pub old_password: String,
    pub new_password: String,
    /// Retained history depth applied on rotation; zero clears history.
    #[serde(default)]
    pub history_limit: usize,
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
    /// Retained previous password hashes for reuse detection.
    #[serde(default)]
    pub password_history: Vec<String>,
}

impl UserInfo {
    /// Create a new user (using plaintext passwords, internal autohashing)
    pub fn new(username: String, password: String) -> Result<Self, crate::StorageError> {
        let password_hash = bcrypt::hash(password, bcrypt_cost()).map_err(|e| {
            crate::StorageError::db_error(format!("Password encryption failed: {}", e))
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
            password_history: Vec::new(),
        })
    }

    /// Verify Password
    pub fn verify_password(&self, password: &str) -> bool {
        verify_password_hash(password, &self.password_hash)
    }

    /// Whether a plaintext password matches the current or a retained hash.
    pub fn reuses_password(&self, password: &str) -> bool {
        if self.verify_password(password) {
            return true;
        }
        self.password_history
            .iter()
            .any(|hash| verify_password_hash(password, hash))
    }

    /// change your password
    ///
    /// Change the password while rotating the retained history.
    ///
    /// The replaced hash is pushed to the history; a non-zero limit truncates
    /// to that depth, zero clears the history.
    pub fn change_password_with_history(
        &mut self,
        new_password: String,
        history_limit: usize,
    ) -> Result<(), crate::StorageError> {
        let new_hash = bcrypt::hash(new_password, bcrypt_cost()).map_err(|e| {
            crate::StorageError::db_error(format!("Password encryption failed: {}", e))
        })?;
        let old_hash = std::mem::replace(&mut self.password_hash, new_hash);
        if history_limit > 0 {
            self.password_history.push(old_hash);
            let excess = self.password_history.len().saturating_sub(history_limit);
            self.password_history.drain(..excess);
        } else {
            self.password_history.clear();
        }
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
    /// New password (plain text, hashed before storage)
    pub new_password: Option<String>,
    /// Retained history depth applied when the password rotates; zero
    /// clears the history.
    #[serde(default)]
    pub history_limit: usize,
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
            new_password: None,
            history_limit: 0,
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
