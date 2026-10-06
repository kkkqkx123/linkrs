use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::query::executor::streaming::StreamingQueryResult;
use crate::session::ClientSession;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::error::GraphServiceError;
use super::GraphService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > GraphService<S>
{
    /// Execute a query and return a [`StreamingQueryResult`] for chunk-at-a-time consumption.
    pub async fn execute_stream(
        &self,
        session_id: i64,
        stmt: &str,
    ) -> Result<StreamingQueryResult, GraphServiceError> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| GraphServiceError::new(format!("Invalid session ID: {}", session_id)))?;
        let snapshot = session.variables_snapshot();
        self.build_streaming_result(&session, stmt, None, Some(snapshot))
            .await
    }

    /// Execute a query with caller-supplied bindings and return a streaming handle.
    pub async fn execute_stream_with_params(
        &self,
        session_id: i64,
        stmt: &str,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<StreamingQueryResult, GraphServiceError> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| GraphServiceError::new(format!("Invalid session ID: {}", session_id)))?;
        self.build_streaming_result(&session, stmt, parameters, session_variables)
            .await
    }

    /// Shared streaming setup behind [`execute_stream`](Self::execute_stream) and
    /// [`execute_stream_with_params`](Self::execute_stream_with_params).
    pub(crate) async fn build_streaming_result(
        &self,
        session: &Arc<ClientSession>,
        stmt: &str,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<StreamingQueryResult, GraphServiceError> {
        let session_id = session.id();
        if Self::is_config_statement(stmt) {
            return Err(GraphServiceError::new(
                "UPDATE CONFIGS and SHOW CONFIGS are not supported on streaming endpoints; use the unary query endpoint instead",
            ));
        }
        match Self::parse_command(stmt) {
            Err(parse_error) => return Err(parse_error),
            Ok(Some(_)) => {
                return self
                    .execute_with_params(session_id, stmt, parameters, session_variables)
                    .await
                    .map(|result| StreamingQueryResult::from_execution_result(result.execution));
            }
            Ok(None) => {}
        }

        let query_id = self.next_query_id.fetch_add(1, Ordering::Relaxed) as u32;
        let query_request = graphdb_api::api_core::QueryRequest {
            isolation_level: None,
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: session.is_auto_commit(),
            transaction_id: session.current_transaction(),
            parameters,
            session_variables,
            query_id: Some(query_id as u64),
            parsed_statement: None,
            consistency: Default::default(),
        };

        let mut query_api = self.query_api.write();
        let result = if let Some(txn_id) = session.current_transaction() {
            let manager = self
                .transaction_manager
                .as_ref()
                .ok_or_else(|| GraphServiceError::new("Transaction manager is not configured"))?;
            manager
                .refresh_statement_snapshot(txn_id)
                .map_err(|error| GraphServiceError::new(error.to_string()))?;
            let execution = manager
                .create_execution(txn_id, false)
                .map_err(|error| GraphServiceError::new(error.to_string()))?;
            query_api
                .execute_stream_with_execution(stmt, query_request, &execution)
                .map_err(|e| GraphServiceError::from_core_error_with_query(e, stmt))?
        } else {
            let execution_storage = self
                .storage
                .bind_auto_commit_context()
                .map_err(|error| GraphServiceError::new(error.to_string()))?;
            query_api
                .execute_stream_with_operation_storage(stmt, query_request, execution_storage)
                .map_err(|e| GraphServiceError::from_core_error_with_query(e, stmt))?
        };

        result.runtime().assign_query_id(query_id as u64);
        result
            .runtime()
            .set_progress_identity(session_id, query_id as i64);
        result
            .runtime()
            .set_progress_rows_interval(self.progress_rows_interval);
        session.register_streaming_query(query_id, stmt.to_string(), result.runtime_downgrade());

        let session_clone = session.clone();
        result.set_on_drop(Box::new(move || {
            session_clone.unregister_streaming_query(query_id);
        }));

        Ok(result)
    }
}
