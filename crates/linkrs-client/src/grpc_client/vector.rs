//! Vector index management, search, and payload operations.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Create vector index.
    pub async fn create_vector_index(
        &self,
        request: CreateVectorIndexRequest,
    ) -> Result<CreateVectorIndexResponse> {
        let mut client = self.client();
        client
            .create_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get vector index.
    pub async fn get_vector_index(
        &self,
        request: GetVectorIndexRequest,
    ) -> Result<GetVectorIndexResponse> {
        let mut client = self.client();
        client
            .get_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List vector indexes.
    pub async fn list_vector_indexes(
        &self,
        request: ListVectorIndexesRequest,
    ) -> Result<ListVectorIndexesResponse> {
        let mut client = self.client();
        client
            .list_vector_indexes(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop vector index.
    pub async fn drop_vector_index(
        &self,
        request: DropVectorIndexRequest,
    ) -> Result<DropVectorIndexResponse> {
        let mut client = self.client();
        client
            .drop_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Search vector.
    pub async fn search_vector(
        &self,
        request: SearchVectorRequest,
    ) -> Result<SearchVectorResponse> {
        let mut client = self.client();
        client
            .search_vector(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Scroll vector.
    pub async fn scroll_vector(
        &self,
        mut request: ScrollVectorRequest,
    ) -> Result<ScrollVectorResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .scroll_vector(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get vector count.
    pub async fn get_vector_count(
        &self,
        mut request: GetVectorCountRequest,
    ) -> Result<GetVectorCountResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_vector_count(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Rebuild vector index.
    pub async fn rebuild_vector_index(
        &self,
        mut request: RebuildVectorIndexRequest,
    ) -> Result<RebuildVectorIndexResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .rebuild_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get vector rebuild status.
    pub async fn get_vector_rebuild_status(
        &self,
        mut request: GetVectorRebuildStatusRequest,
    ) -> Result<GetVectorRebuildStatusResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_vector_rebuild_status(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Clear vector index.
    pub async fn clear_vector_index(
        &self,
        mut request: ClearVectorIndexRequest,
    ) -> Result<ClearVectorIndexResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .clear_vector_index(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get vector point.
    pub async fn get_vector_point(
        &self,
        request: GetVectorPointRequest,
    ) -> Result<GetVectorPointResponse> {
        let mut client = self.client();
        client
            .get_vector_point(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Set vector payload.
    pub async fn set_vector_payload(
        &self,
        request: SetVectorPayloadRequest,
    ) -> Result<SetVectorPayloadResponse> {
        let mut client = self.client();
        client
            .set_vector_payload(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Set vector payload fields.
    pub async fn set_vector_payload_fields(
        &self,
        request: SetVectorPayloadFieldsRequest,
    ) -> Result<SetVectorPayloadFieldsResponse> {
        let mut client = self.client();
        client
            .set_vector_payload_fields(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Delete vector payload.
    pub async fn delete_vector_payload(
        &self,
        request: DeleteVectorPayloadRequest,
    ) -> Result<DeleteVectorPayloadResponse> {
        let mut client = self.client();
        client
            .delete_vector_payload(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
