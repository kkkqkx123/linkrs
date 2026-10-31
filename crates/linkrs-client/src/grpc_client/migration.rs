//! Schema migration planning, execution, and history.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Plan a schema migration between two schema versions.
    pub async fn create_migration_plan(
        &self,
        request: MigratePlanRequest,
    ) -> Result<MigratePlanResponse> {
        let mut client = self.client();
        client
            .migrate_plan(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Execute a previously planned migration.
    pub async fn execute_migration(
        &self,
        mut request: MigrateExecuteRequest,
    ) -> Result<MigrateExecuteResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .migrate_execute(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back an executed migration.
    pub async fn rollback_migration(
        &self,
        mut request: MigrateRollbackRequest,
    ) -> Result<MigrateRollbackResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .migrate_rollback(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch the schema version history of a tag or edge type.
    pub async fn get_schema_version_history(
        &self,
        request: VersionHistoryRequest,
    ) -> Result<VersionHistoryResponse> {
        let mut client = self.client();
        client
            .get_version_history(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Stream migration progress events.
    ///
    /// Returns the raw server stream: consume it with `tonic::Streaming::message`
    /// until it yields `Ok(None)`.
    pub async fn stream_migration_progress(
        &self,
        request: StreamMigrationProgressRequest,
    ) -> Result<tonic::Streaming<MigrationProgressEvent>> {
        let mut client = self.client();
        client
            .stream_migration_progress(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Dry run migration.
    pub async fn dry_run_migration(
        &self,
        mut request: DryRunMigrationRequest,
    ) -> Result<DryRunMigrationResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .dry_run_migration(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get migration history.
    pub async fn get_migration_history(
        &self,
        mut request: GetMigrationHistoryRequest,
    ) -> Result<GetMigrationHistoryResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_migration_history(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get migration status.
    pub async fn get_migration_status(
        &self,
        mut request: GetMigrationStatusRequest,
    ) -> Result<GetMigrationStatusResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_migration_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
