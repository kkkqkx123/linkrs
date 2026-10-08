use std::sync::Arc;

use graphdb_api::api_core::SyncApi;
#[cfg(feature = "vector")]
use graphdb_api::api_core::VectorApi;
use graphdb_metrics::{MetricType, StatsManager};
use graphdb_query::query_manager::QueryManager;

use crate::auth::Authenticator;
use crate::permission::GOD_SPACE_ID;
use crate::query::executor::streaming::transaction_scope::CancelReason;
use crate::session::{SessionError, SessionResult};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::GraphService;

/// Detailed user entry combining stored account state with effective role.
#[derive(Debug, Clone)]
pub struct UserDetail {
    pub username: String,
    pub role: Option<String>,
    pub status: String,
    pub last_active: Option<String>,
}

fn format_last_active(millis: Option<i64>) -> Option<String> {
    millis.and_then(|ts| chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339()))
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > GraphService<S>
{
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Arc<crate::session::ClientSession>, String> {
        if username.is_empty() || password.is_empty() {
            self.stats_manager
                .add_value(MetricType::NumAuthFailedSessions);
            return Err("User name or password cannot be empty".to_string());
        }

        if self.session_manager.is_out_of_connections().await {
            self.stats_manager
                .add_value(MetricType::NumAuthFailedSessions);
            return Err("More than the maximum number of connections limit".to_string());
        }

        let auth = self.authenticator.clone();
        let owned_user = username.to_string();
        let owned_pass = password.to_string();
        let auth_result =
            tokio::task::spawn_blocking(move || auth.authenticate(&owned_user, &owned_pass))
                .await
                .map_err(|e| format!("authentication failure: blocking join failed: {}", e))?;

        match auth_result {
            Ok(_) => {
                let _ = self.storage.update_last_login(username);
                let session = self
                    .session_manager
                    .create_session(username.to_string(), "127.0.0.1".to_string())
                    .await
                    .map_err(|e| format!("Creating a session failed: {}", e))?;

                Ok(session)
            }
            Err(e) => {
                self.stats_manager
                    .add_value(MetricType::NumAuthFailedSessions);
                Err(format!("authentication failure: {}", e))
            }
        }
    }

    /// Password-policy settings snapshotted at startup.
    pub fn security_config(&self) -> &crate::config::SecurityConfig {
        &self.security_config
    }

    /// Validate a new plaintext password against the configured policy.
    ///
    /// Applies on every write path, including auth-disabled deployments:
    /// disabling auth only skips the login step, never weak-password writes.
    pub fn validate_new_password(&self, username: &str, password: &str) -> Result<(), String> {
        if password.is_empty() {
            return Err("password cannot be empty".to_string());
        }
        self.security_config
            .password_policy
            .validate_password(username, password)
            .map_err(|reason| format!("password does not meet the password policy: {}", reason))?;
        self.reject_reused_password(username, password)?;
        Ok(())
    }

    /// Whether the account password is past the configured maximum age.
    pub fn password_expired(&self, username: &str) -> bool {
        let max_age_days = self.security_config.password_policy.max_age_days;
        if max_age_days == 0 {
            return false;
        }
        match self.storage.get_user(username) {
            Some(user) => {
                let now = chrono::Utc::now().timestamp_millis();
                let age_millis = now.saturating_sub(user.password_changed_at);
                age_millis > (max_age_days as i64).saturating_mul(86_400_000)
            }
            None => false,
        }
    }

    /// Reject a new password that matches the current or a retained history hash.
    fn reject_reused_password(&self, username: &str, password: &str) -> Result<(), String> {
        if self.security_config.password_policy.history_size == 0 {
            return Ok(());
        }
        let Some(user) = self.storage.get_user(username) else {
            return Ok(());
        };
        if user.reuses_password(password) {
            return Err("password must not reuse a recent password".to_string());
        }
        Ok(())
    }

    /// Session idle timeout driving expiry responses and reclamation.
    pub fn session_idle_timeout(&self) -> std::time::Duration {
        self.session_manager.idle_timeout()
    }

    /// Whether the login response should prompt a password change.
    ///
    /// True when forced rotation is configured and the default seed account
    /// has never changed its password, or when the account password is past
    /// the configured maximum age.
    pub fn must_change_password(&self, username: &str) -> bool {
        if self.is_auth_disabled() {
            return false;
        }
        if self.password_expired(username) {
            return true;
        }
        if !self.authenticator.config().force_change_default_password {
            return false;
        }
        if username != self.authenticator.config().default_username {
            return false;
        }
        match self.storage.get_user(username) {
            Some(user) => user.password_changed_at <= user.created_at,
            None => false,
        }
    }

    /// Whether the account is currently disabled.
    pub fn is_user_locked(&self, username: &str) -> bool {
        self.storage
            .get_user(username)
            .map(|user| user.is_locked)
            .unwrap_or(false)
    }

    /// Graceful shutdown of the shared execution infrastructure.
    pub fn shutdown(&self) {
        let cancelled = self.query_registry.cancel_all(CancelReason::Shutdown);
        if !cancelled.is_empty() {
            log::info!(
                "Cancelled {} active query(s) during shutdown",
                cancelled.len()
            );
        }
        self.shared_scheduler.shutdown_shared();
        log::info!("Shared query scheduler shut down");
    }

    pub async fn signout(&self, session_id: i64) {
        if let Some(session) = self.session_manager.find_session(session_id) {
            if let Some(space_name) = session.space_name() {
                self.stats_manager
                    .dec_space_metric(&space_name, MetricType::NumActiveQueries);
            }
        }
        self.session_manager.remove_session(session_id).await;
    }

    pub fn get_session_manager(&self) -> &Arc<crate::session::GraphSessionManager> {
        &self.session_manager
    }

    pub fn get_permission_manager(&self) -> &Arc<crate::permission::PermissionManager> {
        &self.permission_manager
    }

    pub fn permission_checker(&self) -> crate::permission::PermissionChecker {
        crate::permission::PermissionChecker::new(
            (*self.permission_manager).clone(),
            self.authenticator.config().clone(),
        )
    }

    /// Whether authentication is globally disabled.
    ///
    /// Returns true when either `[auth].enable_authorize = false` or
    /// `[bootstrap].single_user_mode = true`. Both flags cause the
    /// middleware layer to skip session-id verification and fall through to a
    /// default identity so purely-local deployments can operate without a
    /// login round-trip.
    pub fn is_auth_disabled(&self) -> bool {
        !self.authenticator.config().enable_authorize || self.bootstrap_config.single_user_mode
    }

    /// Authenticator configuration reference.
    pub fn auth_config(&self) -> &crate::config::AuthConfig {
        self.authenticator.config()
    }

    /// Bootstrap configuration reference.
    pub fn bootstrap_config(&self) -> &crate::config::BootstrapConfig {
        &self.bootstrap_config
    }

    pub fn get_storage_space_id(&self, space_name: &str) -> Option<i64> {
        self.storage
            .get_space_id(space_name)
            .map(|id| id as i64)
            .ok()
    }

    pub fn get_stats_manager(&self) -> &Arc<StatsManager> {
        &self.stats_manager
    }

    #[cfg(feature = "vector")]
    pub fn vector_api(&self) -> Option<&Arc<VectorApi>> {
        self.vector_api.as_ref()
    }

    pub fn sync_api(&self) -> Option<&Arc<SyncApi>> {
        self.sync_api.as_ref()
    }

    /// Shared process-level query manager.
    pub fn query_manager(&self) -> Arc<QueryManager> {
        Arc::clone(&self.query_manager)
    }

    /// Obtain the session list (SHOW SESSIONS)
    pub async fn list_sessions(&self) -> Vec<crate::session::SessionInfo> {
        self.session_manager.list_sessions().await
    }

    /// Obtain detailed information about the specified session.
    pub async fn get_session_info(&self, session_id: i64) -> Option<crate::session::SessionInfo> {
        self.session_manager.get_session_info(session_id).await
    }

    /// Terminate the session (KILL SESSION)
    pub async fn kill_session(&self, session_id: i64, current_user: &str) -> SessionResult<()> {
        let is_admin = self.permission_manager.is_admin(current_user);

        self.session_manager
            .kill_session(session_id, current_user, is_admin)
            .await
    }

    /// Terminate the query (KILL QUERY)
    pub fn kill_query(&self, session_id: i64, query_id: u32) -> SessionResult<()> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or(SessionError::session_not_found(session_id))?;

        match session.kill_query(query_id) {
            Ok(()) => {
                self.stats_manager.dec_value(MetricType::NumActiveQueries);
                Ok(())
            }
            Err(e) => Err(SessionError::manager_error(e.to_string())),
        }
    }

    fn require_admin(&self, caller: &str) -> Result<(), String> {
        if self.permission_manager.is_admin(caller) {
            Ok(())
        } else {
            Err("admin permission required".to_string())
        }
    }

    fn enabled_admin_count_excluding(&self, exclude: &str) -> usize {
        self.storage
            .list_users()
            .into_iter()
            .filter(|name| name != exclude)
            .filter(|name| self.permission_manager.is_admin(name))
            .filter(|name| {
                self.storage
                    .get_user(name)
                    .map(|user| !user.is_locked)
                    .unwrap_or(false)
            })
            .count()
    }

    /// List users with display role, enablement status and last login time.
    pub fn list_users_detailed(&self) -> Vec<UserDetail> {
        let mut details: Vec<UserDetail> = self
            .storage
            .list_users()
            .into_iter()
            .map(|username| {
                let stored = self.storage.get_user(&username);
                let role = self
                    .permission_manager
                    .highest_role(&username)
                    .map(|role| role.to_string());
                let status = match stored.as_ref() {
                    Some(user) if user.is_locked => "disabled".to_string(),
                    _ => "enabled".to_string(),
                };
                let last_active = stored.and_then(|user| format_last_active(user.last_login_at));
                UserDetail {
                    username,
                    role,
                    status,
                    last_active,
                }
            })
            .collect();
        details.sort_by(|a, b| a.username.cmp(&b.username));
        details
    }

    /// Create a user through the admin channel.
    pub fn admin_create_user(
        &self,
        caller: &str,
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        self.require_admin(caller)?;
        if username.is_empty() || password.is_empty() {
            return Err("username and password cannot be empty".to_string());
        }
        if self.storage.user_exists(username) {
            return Err(format!("user {} already exists", username));
        }
        self.validate_new_password(username, password)?;
        let info = graphdb_core::types::UserInfo::new(username.to_string(), password.to_string())
            .map_err(|e| e.to_string())?;
        let mut handle = (*self.storage).clone();
        handle.create_user(&info).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Reset a password through the admin channel without old-password check.
    ///
    /// `except_session_id` keeps the caller's own session alive while every
    /// other session of the target account is invalidated.
    pub async fn admin_reset_password(
        &self,
        caller: &str,
        caller_session_id: Option<i64>,
        username: &str,
        new_password: &str,
    ) -> Result<(), String> {
        self.require_admin(caller)?;
        if new_password.is_empty() {
            return Err("password cannot be empty".to_string());
        }
        if !self.storage.user_exists(username) {
            return Err(format!("user {} not found", username));
        }
        self.validate_new_password(username, new_password)?;
        let mut alter = graphdb_core::types::UserAlterInfo::new(username.to_string());
        alter.new_password = Some(new_password.to_string());
        alter.history_limit = self.security_config.password_policy.history_size;
        let mut handle = (*self.storage).clone();
        handle.alter_user(&alter).map_err(|e| e.to_string())?;
        self.session_manager
            .remove_sessions_by_username(username, caller_session_id)
            .await;
        Ok(())
    }

    /// Enable or disable a user; disabling evicts existing sessions.
    pub async fn admin_set_user_enabled(
        &self,
        caller: &str,
        caller_session_id: Option<i64>,
        username: &str,
        enabled: bool,
    ) -> Result<(), String> {
        self.require_admin(caller)?;
        if username == caller && !enabled {
            return Err("cannot disable self".to_string());
        }
        let stored = self
            .storage
            .get_user(username)
            .ok_or_else(|| format!("user {} not found", username))?;
        if stored.is_locked == !enabled {
            return Ok(());
        }
        if !enabled
            && self.permission_manager.is_admin(username)
            && self.enabled_admin_count_excluding(username) == 0
        {
            return Err("cannot disable the last available admin".to_string());
        }
        let mut alter = graphdb_core::types::UserAlterInfo::new(username.to_string());
        alter.is_locked = Some(!enabled);
        let mut handle = (*self.storage).clone();
        handle.alter_user(&alter).map_err(|e| e.to_string())?;
        if !enabled {
            self.session_manager
                .remove_sessions_by_username(username, caller_session_id)
                .await;
        }
        Ok(())
    }

    /// Drop a user and clean roles plus sessions.
    pub async fn admin_drop_user(&self, caller: &str, username: &str) -> Result<(), String> {
        self.require_admin(caller)?;
        if username == caller {
            return Err("cannot drop self".to_string());
        }
        if !self.storage.user_exists(username) {
            return Err(format!("user {} not found", username));
        }
        if self.permission_manager.is_admin(username)
            && self.enabled_admin_count_excluding(username) == 0
        {
            return Err("cannot drop the last available admin".to_string());
        }
        let mut handle = (*self.storage).clone();
        handle.drop_user(username).map_err(|e| e.to_string())?;
        self.permission_manager.remove_user(username);
        self.session_manager
            .remove_sessions_by_username(username, None)
            .await;
        Ok(())
    }

    fn resolve_space_id(&self, space: &str) -> Result<i64, String> {
        let trimmed = space.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("global")
            || trimmed == GOD_SPACE_ID.to_string()
        {
            return Ok(GOD_SPACE_ID);
        }
        self.storage
            .get_space_id(trimmed)
            .map(|id| id as i64)
            .map_err(|e| e.to_string())
    }

    /// Grant a role with dual write to storage and permission state.
    pub fn admin_grant_role(
        &self,
        caller: &str,
        username: &str,
        space: &str,
        role_name: &str,
    ) -> Result<(), String> {
        if caller == username {
            return Err("cannot modify own role".to_string());
        }
        if !self.storage.user_exists(username) {
            return Err(format!("user {} not found", username));
        }
        let role: graphdb_core::RoleType = role_name.parse().map_err(|e: String| e)?;
        let space_id = self.resolve_space_id(space)?;
        let operator_role = self
            .permission_manager
            .get_role(caller, space_id)
            .or_else(|| self.permission_manager.get_role(caller, GOD_SPACE_ID))
            .ok_or_else(|| "permission denied: only Admin or God can manage roles".to_string())?;
        if !matches!(
            operator_role,
            graphdb_core::RoleType::God
                | graphdb_core::RoleType::Admin
                | graphdb_core::RoleType::Dba
        ) {
            return Err("permission denied: only Admin or God can manage roles".to_string());
        }
        if !operator_role.can_grant(role) {
            return Err(format!("permission denied: cannot grant role {}", role));
        }
        let mut handle = (*self.storage).clone();
        handle
            .grant_role(username, space_id, role)
            .map_err(|e| e.to_string())?;
        self.permission_manager
            .grant_role(username, space_id, role)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Revoke a role with dual write to storage and permission state.
    pub fn admin_revoke_role(
        &self,
        caller: &str,
        username: &str,
        space: &str,
    ) -> Result<(), String> {
        if caller == username {
            return Err("cannot modify own role".to_string());
        }
        if !self.storage.user_exists(username) {
            return Err(format!("user {} not found", username));
        }
        let space_id = self.resolve_space_id(space)?;
        let operator_role = self
            .permission_manager
            .get_role(caller, space_id)
            .or_else(|| self.permission_manager.get_role(caller, GOD_SPACE_ID))
            .ok_or_else(|| "permission denied: only Admin or God can manage roles".to_string())?;
        if !matches!(
            operator_role,
            graphdb_core::RoleType::God
                | graphdb_core::RoleType::Admin
                | graphdb_core::RoleType::Dba
        ) {
            return Err("permission denied: only Admin or God can manage roles".to_string());
        }
        let mut handle = (*self.storage).clone();
        handle
            .revoke_role(username, space_id)
            .map_err(|e| e.to_string())?;
        self.permission_manager
            .revoke_role(username, space_id)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Sync query-channel role writes into permission state.
    ///
    /// Storage remains the source of truth; this keeps the in-memory
    /// permission map consistent when GRANT/REVOKE arrive via queries.
    pub fn sync_role_from_storage(&self, username: &str) {
        let entries = self.storage.list_user_roles(username);
        if entries.is_empty() {
            self.permission_manager.remove_user(username);
            return;
        }
        for (space_id, role) in entries {
            let _ = self.permission_manager.grant_role(username, space_id, role);
        }
    }
}
