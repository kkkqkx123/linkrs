//! Query execution, cursors, and batch query execution.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Execute a query (parameters travel inside the request message).
    pub async fn execute_query(
        &self,
        mut request: ExecuteQueryRequest,
    ) -> Result<ExecuteQueryResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .execute_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Validate a query without executing it.
    pub async fn validate_query(
        &self,
        mut request: ValidateQueryRequest,
    ) -> Result<ValidateQueryResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .validate_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Return the query plan for a query.
    pub async fn explain_query(
        &self,
        mut request: ExplainQueryRequest,
    ) -> Result<ExplainQueryResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .explain_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Execute a query and stream result chunks.
    ///
    /// Returns the raw server stream: consume it with `tonic::Streaming::message`
    /// until it yields `Ok(None)`.
    pub async fn execute_query_stream(
        &self,
        mut request: ExecuteQueryRequest,
    ) -> Result<tonic::Streaming<StreamResponse>> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .execute_query_stream(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List currently running queries.
    pub async fn list_queries(&self) -> Result<ListQueriesResponse> {
        let mut client = self.client();
        client
            .list_queries(self.authed(tonic::Request::new(ListQueriesRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Kill a running query.
    pub async fn kill_query(&self, mut request: KillQueryRequest) -> Result<KillQueryResponse> {
        self.fill_optional_session_i64(&mut request.session_id)?;
        let mut client = self.client();
        client
            .kill_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Open a cursor for paged query result retrieval.
    pub async fn open_cursor(&self, mut request: OpenCursorRequest) -> Result<OpenCursorResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .open_cursor(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch the next page of a cursor.
    pub async fn fetch_cursor(
        &self,
        mut request: FetchCursorRequest,
    ) -> Result<FetchCursorResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .fetch_cursor(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Close a cursor and release its server-side resources.
    pub async fn close_cursor(
        &self,
        mut request: CloseCursorRequest,
    ) -> Result<CloseCursorResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .close_cursor(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Execute several queries in one call.
    pub async fn execute_batch_query(
        &self,
        mut request: ExecuteBatchQueryRequest,
    ) -> Result<ExecuteBatchQueryResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .execute_batch_query(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
