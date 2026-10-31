//! Custom function registration and inspection.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Register function.
    pub async fn register_function(
        &self,
        request: RegisterFunctionRequest,
    ) -> Result<RegisterFunctionResponse> {
        let mut client = self.client();
        client
            .register_function(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Unregister function.
    pub async fn unregister_function(
        &self,
        request: UnregisterFunctionRequest,
    ) -> Result<UnregisterFunctionResponse> {
        let mut client = self.client();
        client
            .unregister_function(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List functions.
    pub async fn list_functions(&self) -> Result<ListFunctionsResponse> {
        let mut client = self.client();
        client
            .list_functions(self.authed(tonic::Request::new(ListFunctionsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get function info.
    pub async fn get_function_info(
        &self,
        request: GetFunctionInfoRequest,
    ) -> Result<GetFunctionInfoResponse> {
        let mut client = self.client();
        client
            .get_function_info(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
