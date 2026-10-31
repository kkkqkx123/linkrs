//! Session, query, database, and subsystem statistics.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Fetch session statistics.
    pub async fn get_session_statistics(
        &self,
        mut request: GetSessionStatisticsRequest,
    ) -> Result<GetSessionStatisticsResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .get_session_statistics(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch query statistics.
    pub async fn get_query_statistics(
        &self,
        mut request: GetQueryStatisticsRequest,
    ) -> Result<GetQueryStatisticsResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .get_query_statistics(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch database statistics.
    pub async fn get_database_statistics(&self) -> Result<GetDatabaseStatisticsResponse> {
        let mut client = self.client();
        client
            .get_database_statistics(
                self.authed(tonic::Request::new(GetDatabaseStatisticsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch system statistics.
    pub async fn get_system_statistics(&self) -> Result<GetSystemStatisticsResponse> {
        let mut client = self.client();
        client
            .get_system_statistics(self.authed(tonic::Request::new(GetSystemStatisticsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get overview statistics.
    pub async fn get_overview_statistics(&self) -> Result<GetOverviewStatisticsResponse> {
        let mut client = self.client();
        client
            .get_overview_statistics(
                self.authed(tonic::Request::new(GetOverviewStatisticsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get search statistics.
    pub async fn get_search_statistics(&self) -> Result<GetSearchStatisticsResponse> {
        let mut client = self.client();
        client
            .get_search_statistics(self.authed(tonic::Request::new(GetSearchStatisticsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get migration statistics.
    pub async fn get_migration_statistics(&self) -> Result<GetMigrationStatisticsResponse> {
        let mut client = self.client();
        client
            .get_migration_statistics(
                self.authed(tonic::Request::new(GetMigrationStatisticsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get freeze statistics.
    pub async fn get_freeze_statistics(&self) -> Result<GetFreezeStatisticsResponse> {
        let mut client = self.client();
        client
            .get_freeze_statistics(self.authed(tonic::Request::new(GetFreezeStatisticsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Trigger freeze.
    pub async fn trigger_freeze(&self) -> Result<TriggerFreezeResponse> {
        let mut client = self.client();
        client
            .trigger_freeze(self.authed(tonic::Request::new(TriggerFreezeRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get query profile detail.
    pub async fn get_query_profile_detail(
        &self,
        request: GetQueryProfileDetailRequest,
    ) -> Result<GetQueryProfileDetailResponse> {
        let mut client = self.client();
        client
            .get_query_profile_detail(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
