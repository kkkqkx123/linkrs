//! Fulltext index maintenance and search.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Rebuild fulltext index.
    pub async fn rebuild_fulltext_index(
        &self,
        mut request: RebuildFulltextIndexRequest,
    ) -> Result<RebuildFulltextIndexResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .rebuild_fulltext_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get fulltext rebuild status.
    pub async fn get_fulltext_rebuild_status(
        &self,
        mut request: GetFulltextRebuildStatusRequest,
    ) -> Result<GetFulltextRebuildStatusResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_fulltext_rebuild_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Clear fulltext index.
    pub async fn clear_fulltext_index(
        &self,
        mut request: ClearFulltextIndexRequest,
    ) -> Result<ClearFulltextIndexResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .clear_fulltext_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List inconsistent fulltext.
    pub async fn list_inconsistent_fulltext(
        &self,
        mut request: ListInconsistentFulltextRequest,
    ) -> Result<ListInconsistentFulltextResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .list_inconsistent_fulltext(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Search fulltext.
    pub async fn search_fulltext(
        &self,
        mut request: SearchFulltextRequest,
    ) -> Result<SearchFulltextResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .search_fulltext(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
