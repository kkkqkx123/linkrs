use std::sync::Arc;

use graphdb_api::api_core::SyncApi;
#[cfg(feature = "vector")]
use graphdb_api::api_core::VectorApi;
use graphdb_metrics::{MetricType, StatsManager};
use graphdb_query::query_manager::QueryManager;

use crate::query::executor::streaming::transaction_scope::CancelReason;
use crate::auth::Authenticator;
use crate::session::{SessionError, SessionResult};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::GraphService;

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

        match self.authenticator.authenticate(username, password) {
            Ok(_) => {
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
        let current_session = self
            .session_manager
            .find_session(session_id)
            .ok_or(SessionError::session_not_found(session_id))?;

        let is_admin = current_session.is_admin();

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
}
