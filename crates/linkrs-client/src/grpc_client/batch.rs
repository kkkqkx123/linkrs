//! Batch job lifecycle management.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Create a batch job.
    pub async fn create_batch(&self, request: CreateBatchRequest) -> Result<CreateBatchResponse> {
        let mut client = self.client();
        client
            .create_batch(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Append items to a batch job.
    pub async fn add_batch_items(
        &self,
        request: AddBatchItemsRequest,
    ) -> Result<AddBatchItemsResponse> {
        let mut client = self.client();
        client
            .add_batch_items(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Execute a batch job.
    pub async fn execute_batch(
        &self,
        request: ExecuteBatchRequest,
    ) -> Result<ExecuteBatchResponse> {
        let mut client = self.client();
        client
            .execute_batch(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch batch job status.
    pub async fn get_batch_status(
        &self,
        request: GetBatchStatusRequest,
    ) -> Result<GetBatchStatusResponse> {
        let mut client = self.client();
        client
            .get_batch_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Cancel a batch job.
    pub async fn cancel_batch(&self, request: CancelBatchRequest) -> Result<CancelBatchResponse> {
        let mut client = self.client();
        client
            .cancel_batch(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Delete batch.
    pub async fn delete_batch(&self, request: DeleteBatchRequest) -> Result<DeleteBatchResponse> {
        let mut client = self.client();
        client
            .delete_batch(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
