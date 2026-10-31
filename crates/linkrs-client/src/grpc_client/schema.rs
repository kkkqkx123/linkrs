//! Space, tag, edge type, and schema version management.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Create a space.
    pub async fn create_space(&self, request: CreateSpaceRequest) -> Result<CreateSpaceResponse> {
        let mut client = self.client();
        client
            .create_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch a space by name.
    pub async fn get_space(&self, request: GetSpaceRequest) -> Result<GetSpaceResponse> {
        let mut client = self.client();
        client
            .get_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop a space.
    pub async fn drop_space(&self, request: DropSpaceRequest) -> Result<DropSpaceResponse> {
        let mut client = self.client();
        client
            .drop_space(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List spaces.
    pub async fn list_spaces(&self) -> Result<ListSpacesResponse> {
        let mut client = self.client();
        client
            .list_spaces(self.authed(tonic::Request::new(ListSpacesRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create a tag.
    pub async fn create_tag(&self, request: CreateTagRequest) -> Result<CreateTagResponse> {
        let mut client = self.client();
        client
            .create_tag(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch a tag definition.
    pub async fn get_tag(&self, request: GetTagRequest) -> Result<GetTagResponse> {
        let mut client = self.client();
        client
            .get_tag(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List tags in a space.
    pub async fn list_tags(&self, request: ListTagsRequest) -> Result<ListTagsResponse> {
        let mut client = self.client();
        client
            .list_tags(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop a tag.
    pub async fn drop_tag(&self, request: DropTagRequest) -> Result<DropTagResponse> {
        let mut client = self.client();
        client
            .drop_tag(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create an edge type.
    pub async fn create_edge_type(
        &self,
        request: CreateEdgeTypeRequest,
    ) -> Result<CreateEdgeTypeResponse> {
        let mut client = self.client();
        client
            .create_edge_type(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch an edge type definition.
    pub async fn get_edge_type(&self, request: GetEdgeTypeRequest) -> Result<GetEdgeTypeResponse> {
        let mut client = self.client();
        client
            .get_edge_type(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List edge types in a space.
    pub async fn list_edge_types(
        &self,
        request: ListEdgeTypesRequest,
    ) -> Result<ListEdgeTypesResponse> {
        let mut client = self.client();
        client
            .list_edge_types(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop an edge type.
    pub async fn drop_edge_type(
        &self,
        request: DropEdgeTypeRequest,
    ) -> Result<DropEdgeTypeResponse> {
        let mut client = self.client();
        client
            .drop_edge_type(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Get schema changes.
    pub async fn get_schema_changes(
        &self,
        request: SchemaChangesRequest,
    ) -> Result<SchemaChangesResponse> {
        let mut client = self.client();
        client
            .get_schema_changes(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Detect breaking changes.
    pub async fn detect_breaking_changes(
        &self,
        request: BreakingChangesRequest,
    ) -> Result<BreakingChangesResponse> {
        let mut client = self.client();
        client
            .detect_breaking_changes(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
