//! Data import and export.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Import data into a space.
    pub async fn import_data(&self, mut request: ImportDataRequest) -> Result<ImportDataResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .import_data(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch the status of an import job.
    pub async fn get_import_status(
        &self,
        request: GetImportStatusRequest,
    ) -> Result<GetImportStatusResponse> {
        let mut client = self.client();
        client
            .get_import_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Export query results in the requested format.
    pub async fn export_data(&self, mut request: ExportDataRequest) -> Result<ExportDataResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .export_data(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
