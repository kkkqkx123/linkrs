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
}
