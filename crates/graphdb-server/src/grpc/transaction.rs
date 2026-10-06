//! Transaction and savepoint handlers.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use graphdb_transaction::{DurabilityLevel, IsolationLevel, TransactionOptions};

use super::error::{parse_transaction_id, transaction_status};
use super::proto::*;
use super::service::GraphDBService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > GraphDBService<S>
{
    pub(crate) async fn handle_begin_transaction(
        &self,
        request: Request<BeginTransactionRequest>,
    ) -> Result<Response<BeginTransactionResponse>, Status> {
        let request = request.into_inner();
        let proto_options = request.options.unwrap_or_default();
        let isolation_level = match proto_options.isolation_level {
            0 => IsolationLevel::RepeatableRead,
            1 => IsolationLevel::ReadCommitted,
            2 => IsolationLevel::Serializable,
            value => {
                return Err(Status::invalid_argument(format!(
                    "Unsupported isolation level: {}",
                    value
                )))
            }
        };
        let options = TransactionOptions {
            timeout: (proto_options.timeout_ms > 0).then_some(std::time::Duration::from_millis(
                proto_options.timeout_ms as u64,
            )),
            read_only: proto_options.read_only,
            long_read: false,
            durability: DurabilityLevel::Sync,
            isolation_level,
            query_timeout: None,
            statement_timeout: None,
            idle_timeout: None,
        };
        let manager = self.app_state.server.get_txn_manager();
        let txn_id = match request.session_id {
            Some(owner) => manager.begin_transaction_with_owner(options, owner),
            None => manager.begin_transaction(options),
        }
        .map_err(transaction_status)?;

        Ok(Response::new(BeginTransactionResponse {
            success: true,
            transaction_id: txn_id.as_u64().to_string(),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_commit_transaction(
        &self,
        request: Request<CommitTransactionRequest>,
    ) -> Result<Response<CommitTransactionResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        manager
            .commit_transaction(txn_id)
            .map_err(transaction_status)?;
        Ok(Response::new(CommitTransactionResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_rollback_transaction(
        &self,
        request: Request<RollbackTransactionRequest>,
    ) -> Result<Response<RollbackTransactionResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        manager
            .abort_transaction(txn_id)
            .map_err(transaction_status)?;
        Ok(Response::new(RollbackTransactionResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_create_savepoint(
        &self,
        request: Request<CreateSavepointRequest>,
    ) -> Result<Response<CreateSavepointResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        let staged_mark = {
            let storage = self.app_state.server.get_storage();
            let storage_guard = storage.read();
            crate::storage::UndoTarget::staged_write_mark(&*storage_guard, txn_id)
        };
        let savepoint_id = manager
            .create_savepoint(txn_id, request.name, staged_mark)
            .map_err(transaction_status)?;
        Ok(Response::new(CreateSavepointResponse {
            success: true,
            savepoint_id,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_rollback_to_savepoint(
        &self,
        request: Request<RollbackToSavepointRequest>,
    ) -> Result<Response<RollbackToSavepointResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        let storage = self.app_state.server.get_storage();
        let storage_guard = storage.read();
        manager
            .rollback_to_savepoint(txn_id, request.savepoint_id, &*storage_guard)
            .map_err(transaction_status)?;
        Ok(Response::new(RollbackToSavepointResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_list_transactions(
        &self,
        _request: Request<ListTransactionsRequest>,
    ) -> Result<Response<ListTransactionsResponse>, Status> {
        let manager = self.app_state.server.get_txn_manager();
        let transactions = manager
            .list_transactions()
            .into_iter()
            .map(|info| TransactionInfo {
                transaction_id: info.id.as_u64(),
                state: info.state.to_string(),
                owner: info.owner.unwrap_or_default(),
                elapsed_ms: info.elapsed.as_millis() as u64,
                last_activity_ms: info.last_activity.as_millis() as u64,
                rollback_only: info.rollback_only,
                staged_bytes: info.staged_bytes,
                undo_bytes: info.undo_bytes,
            })
            .collect();
        Ok(Response::new(ListTransactionsResponse {
            transactions,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_kill_transaction(
        &self,
        request: Request<KillTransactionRequest>,
    ) -> Result<Response<KillTransactionResponse>, Status> {
        let req = request.into_inner();
        let manager = self.app_state.server.get_txn_manager();
        match manager.kill_transaction(req.transaction_id.into(), req.owner.as_deref()) {
            Ok(()) => Ok(Response::new(KillTransactionResponse {
                success: true,
                error: String::new(),
            })),
            Err(e) => Err(transaction_status(e)),
        }
    }

    pub(crate) async fn handle_get_transaction_metrics(
        &self,
        _request: Request<GetTransactionMetricsRequest>,
    ) -> Result<Response<GetTransactionMetricsResponse>, Status> {
        let manager = self.app_state.server.get_txn_manager();
        let stats = manager.stats();
        let resources = manager.resource_metrics();
        let metrics = serde_json::json!({
            "outcomes": {
                "begun": stats.total_transactions.load(std::sync::atomic::Ordering::Relaxed),
                "active": stats.active_transactions.load(std::sync::atomic::Ordering::Relaxed),
                "committed": stats.committed_transactions.load(std::sync::atomic::Ordering::Relaxed),
                "aborted": stats.aborted_transactions.load(std::sync::atomic::Ordering::Relaxed),
            },
            "resources": {
                "active_statements": stats.active_statements.load(std::sync::atomic::Ordering::Relaxed),
                "active_snapshots": resources.active_snapshots,
                "pending_writes": resources.pending_writes,
                "staged_wal_bytes": resources.staged_wal_bytes,
                "undo_bytes": resources.undo_bytes,
            },
        });
        Ok(Response::new(GetTransactionMetricsResponse {
            metrics_json: serde_json::to_string(&metrics).unwrap_or_else(|_| "{}".to_string()),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_release_savepoint(
        &self,
        request: Request<ReleaseSavepointRequest>,
    ) -> Result<Response<ReleaseSavepointResponse>, Status> {
        let request = request.into_inner();
        let txn_id = parse_transaction_id(&request.transaction_id)?;
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(txn_id, request.session_id.as_deref())
            .map_err(transaction_status)?;
        manager
            .get_context(txn_id)
            .map_err(transaction_status)?
            .release_savepoint(request.savepoint_id)
            .map_err(transaction_status)?;
        Ok(Response::new(ReleaseSavepointResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_list_savepoints(
        &self,
        request: Request<ListSavepointsRequest>,
    ) -> Result<Response<ListSavepointsResponse>, Status> {
        let req = request.into_inner();
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(req.transaction_id.into(), req.owner.as_deref())
            .map_err(transaction_status)?;
        let txn_api = self.app_state.server.get_txn_api();
        let handle =
            graphdb_api::api_core::TransactionHandle::from(req.transaction_id);
        match txn_api.get_savepoints(handle) {
            Ok(savepoints) => Ok(Response::new(ListSavepointsResponse {
                savepoints: savepoints
                    .into_iter()
                    .map(|sp| super::proto::SavepointInfo {
                        savepoint_id: sp.id,
                        name: sp.name.unwrap_or_default(),
                        created_at: format!("{:?}", sp.created_at),
                    })
                    .collect(),
                error: String::new(),
            })),
            Err(e) => Err(Status::internal(format!(
                "Failed to list savepoints: {}",
                e
            ))),
        }
    }

    pub(crate) async fn handle_retry_transaction_outbox(
        &self,
        request: Request<RetryTransactionOutboxRequest>,
    ) -> Result<Response<RetryTransactionOutboxResponse>, Status> {
        let req = request.into_inner();
        let manager = self.app_state.server.get_txn_manager();
        manager
            .check_transaction_owner(req.transaction_id.into(), req.owner.as_deref())
            .map_err(transaction_status)?;
        match manager.retry_outbox_projection() {
            Ok(delivered) => Ok(Response::new(RetryTransactionOutboxResponse {
                transaction_id: req.transaction_id,
                delivered: delivered as u64,
                status: "completed".to_string(),
                error: String::new(),
            })),
            Err(e) => Err(transaction_status(e)),
        }
    }
}
