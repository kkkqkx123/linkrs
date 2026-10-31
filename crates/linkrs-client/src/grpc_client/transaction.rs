//! Transaction lifecycle, savepoints, and transaction outbox retries.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Begin a transaction.
    pub async fn begin_transaction(
        &self,
        mut request: BeginTransactionRequest,
    ) -> Result<BeginTransactionResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .begin_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Commit a transaction.
    pub async fn commit_transaction(
        &self,
        mut request: CommitTransactionRequest,
    ) -> Result<CommitTransactionResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .commit_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back a transaction.
    pub async fn rollback_transaction(
        &self,
        mut request: RollbackTransactionRequest,
    ) -> Result<RollbackTransactionResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .rollback_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create a savepoint inside a transaction.
    pub async fn create_savepoint(
        &self,
        mut request: CreateSavepointRequest,
    ) -> Result<CreateSavepointResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .create_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Roll back to a savepoint.
    pub async fn rollback_to_savepoint(
        &self,
        mut request: RollbackToSavepointRequest,
    ) -> Result<RollbackToSavepointResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .rollback_to_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Release a savepoint.
    pub async fn release_savepoint(
        &self,
        mut request: ReleaseSavepointRequest,
    ) -> Result<ReleaseSavepointResponse> {
        self.fill_optional_session(&mut request.session_id);
        let mut client = self.client();
        client
            .release_savepoint(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List active transactions.
    pub async fn list_transactions(&self) -> Result<ListTransactionsResponse> {
        let mut client = self.client();
        client
            .list_transactions(self.authed(tonic::Request::new(ListTransactionsRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Kill a transaction.
    pub async fn kill_transaction(
        &self,
        mut request: KillTransactionRequest,
    ) -> Result<KillTransactionResponse> {
        self.fill_optional_owner(&mut request.owner);
        let mut client = self.client();
        client
            .kill_transaction(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Fetch transaction manager metrics.
    pub async fn transaction_metrics(&self) -> Result<GetTransactionMetricsResponse> {
        let mut client = self.client();
        client
            .get_transaction_metrics(
                self.authed(tonic::Request::new(GetTransactionMetricsRequest {})),
            )
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List savepoints of a transaction.
    pub async fn list_savepoints(
        &self,
        mut request: ListSavepointsRequest,
    ) -> Result<ListSavepointsResponse> {
        self.fill_optional_owner(&mut request.owner);
        let mut client = self.client();
        client
            .list_savepoints(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Retry transaction outbox.
    pub async fn retry_transaction_outbox(
        &self,
        mut request: RetryTransactionOutboxRequest,
    ) -> Result<RetryTransactionOutboxResponse> {
        self.fill_optional_owner(&mut request.owner);
        let mut client = self.client();
        client
            .retry_transaction_outbox(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}
