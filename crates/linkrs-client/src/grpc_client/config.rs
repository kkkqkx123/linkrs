//! Runtime configuration access.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Fetch the full server configuration.
    pub async fn get_config(&self) -> Result<GetConfigResponse> {
        let mut client = self.client();
        client
            .get_config(self.authed(tonic::Request::new(GetConfigRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch a single configuration key.
    pub async fn get_config_key(
        &self,
        request: GetConfigKeyRequest,
    ) -> Result<GetConfigKeyResponse> {
        let mut client = self.client();
        client
            .get_config_key(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Update a configuration key.
    pub async fn update_config(
        &self,
        request: UpdateConfigRequest,
    ) -> Result<UpdateConfigResponse> {
        let mut client = self.client();
        client
            .update_config(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Reset a configuration key to its default.
    pub async fn reset_config(&self, request: ResetConfigRequest) -> Result<ResetConfigResponse> {
        let mut client = self.client();
        client
            .reset_config(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Update several configuration keys at once.
    pub async fn bulk_update_config(
        &self,
        request: BulkUpdateConfigRequest,
    ) -> Result<BulkUpdateConfigResponse> {
        let mut client = self.client();
        client
            .bulk_update_config(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
