//! Synchronization status, outbox diagnostics, dead letters, and retention.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Get sync status.
    pub async fn get_sync_status(&self) -> Result<GetSyncStatusResponse> {
        let mut client = self.client();
        client
            .get_sync_status(self.authed(tonic::Request::new(GetSyncStatusRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Retry outbox.
    pub async fn retry_outbox(&self) -> Result<RetryOutboxResponse> {
        let mut client = self.client();
        client
            .retry_outbox(self.authed(tonic::Request::new(RetryOutboxRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get outbox diagnostics.
    pub async fn get_outbox_diagnostics(&self) -> Result<GetOutboxDiagnosticsResponse> {
        let mut client = self.client();
        client
            .get_outbox_diagnostics(
                self.authed(tonic::Request::new(GetOutboxDiagnosticsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List dead letters.
    pub async fn list_dead_letters(
        &self,
        request: ListDeadLettersRequest,
    ) -> Result<ListDeadLettersResponse> {
        let mut client = self.client();
        client
            .list_dead_letters(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Requeue dead letters.
    pub async fn requeue_dead_letters(
        &self,
        request: RequeueDeadLettersRequest,
    ) -> Result<RequeueDeadLettersResponse> {
        let mut client = self.client();
        client
            .requeue_dead_letters(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get retention status.
    pub async fn get_retention_status(&self) -> Result<GetRetentionStatusResponse> {
        let mut client = self.client();
        client
            .get_retention_status(self.authed(tonic::Request::new(GetRetentionStatusRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Run retention.
    pub async fn run_retention(
        &self,
        request: RunRetentionRequest,
    ) -> Result<RunRetentionResponse> {
        let mut client = self.client();
        client
            .run_retention(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List degraded ranges.
    pub async fn list_degraded_ranges(
        &self,
        request: ListDegradedRangesRequest,
    ) -> Result<ListDegradedRangesResponse> {
        let mut client = self.client();
        client
            .list_degraded_ranges(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Clear degraded range.
    pub async fn clear_degraded_range(
        &self,
        request: ClearDegradedRangeRequest,
    ) -> Result<ClearDegradedRangeResponse> {
        let mut client = self.client();
        client
            .clear_degraded_range(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
