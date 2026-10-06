//! gRPC trait dispatch.
//!
//! Implements the generated service trait by delegating to responsibility
//! focused handler modules. No business logic lives here.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::proto::graph_db_service_server::GraphDbService as GraphDBServiceTrait;
use super::proto::*;
use super::service::{ExecuteQueryStreamStream, StreamMigrationProgressStream};

pub use super::service::GraphDBService;
pub use super::service::GraphDBService as GrpcService;

#[tonic::async_trait]
impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + crate::storage::AutoCommitBatchOps
            + Clone
            + Send
            + Sync
            + 'static,
    > GraphDBServiceTrait for GraphDBService<S>
{
    type ExecuteQueryStreamStream = ExecuteQueryStreamStream;
    type StreamMigrationProgressStream = StreamMigrationProgressStream;

    async fn health_check(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        self.handle_health_check(request).await
    }

    async fn login(
        &self,
        request: Request<LoginRequest>,
    ) -> Result<Response<LoginResponse>, Status> {
        self.handle_login(request).await
    }

    async fn logout(
        &self,
        request: Request<LogoutRequest>,
    ) -> Result<Response<LogoutResponse>, Status> {
        self.handle_logout(request).await
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        self.handle_create_session(request).await
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        self.handle_get_session(request).await
    }

    async fn close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        self.handle_close_session(request).await
    }

    async fn execute_query(
        &self,
        request: Request<ExecuteQueryRequest>,
    ) -> Result<Response<ExecuteQueryResponse>, Status> {
        self.handle_execute_query(request).await
    }

    async fn validate_query(
        &self,
        request: Request<ValidateQueryRequest>,
    ) -> Result<Response<ValidateQueryResponse>, Status> {
        self.handle_validate_query(request).await
    }

    async fn execute_query_stream(
        &self,
        request: Request<ExecuteQueryRequest>,
    ) -> Result<Response<Self::ExecuteQueryStreamStream>, Status> {
        self.handle_execute_query_stream(request).await
    }

    async fn begin_transaction(
        &self,
        request: Request<BeginTransactionRequest>,
    ) -> Result<Response<BeginTransactionResponse>, Status> {
        self.handle_begin_transaction(request).await
    }

    async fn commit_transaction(
        &self,
        request: Request<CommitTransactionRequest>,
    ) -> Result<Response<CommitTransactionResponse>, Status> {
        self.handle_commit_transaction(request).await
    }

    async fn rollback_transaction(
        &self,
        request: Request<RollbackTransactionRequest>,
    ) -> Result<Response<RollbackTransactionResponse>, Status> {
        self.handle_rollback_transaction(request).await
    }

    async fn create_savepoint(
        &self,
        request: Request<CreateSavepointRequest>,
    ) -> Result<Response<CreateSavepointResponse>, Status> {
        self.handle_create_savepoint(request).await
    }

    async fn rollback_to_savepoint(
        &self,
        request: Request<RollbackToSavepointRequest>,
    ) -> Result<Response<RollbackToSavepointResponse>, Status> {
        self.handle_rollback_to_savepoint(request).await
    }

    async fn release_savepoint(
        &self,
        request: Request<ReleaseSavepointRequest>,
    ) -> Result<Response<ReleaseSavepointResponse>, Status> {
        self.handle_release_savepoint(request).await
    }

    async fn create_space(
        &self,
        request: Request<CreateSpaceRequest>,
    ) -> Result<Response<CreateSpaceResponse>, Status> {
        self.handle_create_space(request).await
    }

    async fn get_space(
        &self,
        request: Request<GetSpaceRequest>,
    ) -> Result<Response<GetSpaceResponse>, Status> {
        self.handle_get_space(request).await
    }

    async fn drop_space(
        &self,
        request: Request<DropSpaceRequest>,
    ) -> Result<Response<DropSpaceResponse>, Status> {
        self.handle_drop_space(request).await
    }

    async fn list_spaces(
        &self,
        request: Request<ListSpacesRequest>,
    ) -> Result<Response<ListSpacesResponse>, Status> {
        self.handle_list_spaces(request).await
    }

    async fn create_tag(
        &self,
        request: Request<CreateTagRequest>,
    ) -> Result<Response<CreateTagResponse>, Status> {
        self.handle_create_tag(request).await
    }

    async fn get_tag(
        &self,
        request: Request<GetTagRequest>,
    ) -> Result<Response<GetTagResponse>, Status> {
        self.handle_get_tag(request).await
    }

    async fn list_tags(
        &self,
        request: Request<ListTagsRequest>,
    ) -> Result<Response<ListTagsResponse>, Status> {
        self.handle_list_tags(request).await
    }

    async fn drop_tag(
        &self,
        request: Request<DropTagRequest>,
    ) -> Result<Response<DropTagResponse>, Status> {
        self.handle_drop_tag(request).await
    }

    async fn create_edge_type(
        &self,
        request: Request<CreateEdgeTypeRequest>,
    ) -> Result<Response<CreateEdgeTypeResponse>, Status> {
        self.handle_create_edge_type(request).await
    }

    async fn get_edge_type(
        &self,
        request: Request<GetEdgeTypeRequest>,
    ) -> Result<Response<GetEdgeTypeResponse>, Status> {
        self.handle_get_edge_type(request).await
    }

    async fn list_edge_types(
        &self,
        request: Request<ListEdgeTypesRequest>,
    ) -> Result<Response<ListEdgeTypesResponse>, Status> {
        self.handle_list_edge_types(request).await
    }

    async fn drop_edge_type(
        &self,
        request: Request<DropEdgeTypeRequest>,
    ) -> Result<Response<DropEdgeTypeResponse>, Status> {
        self.handle_drop_edge_type(request).await
    }

    async fn create_batch(
        &self,
        request: Request<CreateBatchRequest>,
    ) -> Result<Response<CreateBatchResponse>, Status> {
        self.handle_create_batch(request).await
    }

    async fn add_batch_items(
        &self,
        request: Request<AddBatchItemsRequest>,
    ) -> Result<Response<AddBatchItemsResponse>, Status> {
        self.handle_add_batch_items(request).await
    }

    async fn execute_batch(
        &self,
        request: Request<ExecuteBatchRequest>,
    ) -> Result<Response<ExecuteBatchResponse>, Status> {
        self.handle_execute_batch(request).await
    }

    async fn get_batch_status(
        &self,
        request: Request<GetBatchStatusRequest>,
    ) -> Result<Response<GetBatchStatusResponse>, Status> {
        self.handle_get_batch_status(request).await
    }

    async fn cancel_batch(
        &self,
        request: Request<CancelBatchRequest>,
    ) -> Result<Response<CancelBatchResponse>, Status> {
        self.handle_cancel_batch(request).await
    }

    async fn get_session_statistics(
        &self,
        request: Request<GetSessionStatisticsRequest>,
    ) -> Result<Response<GetSessionStatisticsResponse>, Status> {
        self.handle_get_session_statistics(request).await
    }

    async fn get_query_statistics(
        &self,
        request: Request<GetQueryStatisticsRequest>,
    ) -> Result<Response<GetQueryStatisticsResponse>, Status> {
        self.handle_get_query_statistics(request).await
    }

    async fn get_database_statistics(
        &self,
        request: Request<GetDatabaseStatisticsRequest>,
    ) -> Result<Response<GetDatabaseStatisticsResponse>, Status> {
        self.handle_get_database_statistics(request).await
    }

    async fn get_system_statistics(
        &self,
        request: Request<GetSystemStatisticsRequest>,
    ) -> Result<Response<GetSystemStatisticsResponse>, Status> {
        self.handle_get_system_statistics(request).await
    }

    async fn get_config(
        &self,
        request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        self.handle_get_config(request).await
    }

    async fn update_config(
        &self,
        request: Request<UpdateConfigRequest>,
    ) -> Result<Response<UpdateConfigResponse>, Status> {
        self.handle_update_config(request).await
    }

    async fn reset_config(
        &self,
        request: Request<ResetConfigRequest>,
    ) -> Result<Response<ResetConfigResponse>, Status> {
        self.handle_reset_config(request).await
    }

    async fn register_function(
        &self,
        request: Request<RegisterFunctionRequest>,
    ) -> Result<Response<RegisterFunctionResponse>, Status> {
        self.handle_register_function(request).await
    }

    async fn unregister_function(
        &self,
        request: Request<UnregisterFunctionRequest>,
    ) -> Result<Response<UnregisterFunctionResponse>, Status> {
        self.handle_unregister_function(request).await
    }

    async fn list_functions(
        &self,
        request: Request<ListFunctionsRequest>,
    ) -> Result<Response<ListFunctionsResponse>, Status> {
        self.handle_list_functions(request).await
    }

    async fn get_function_info(
        &self,
        request: Request<GetFunctionInfoRequest>,
    ) -> Result<Response<GetFunctionInfoResponse>, Status> {
        self.handle_get_function_info(request).await
    }

    async fn create_vector_index(
        &self,
        request: Request<CreateVectorIndexRequest>,
    ) -> Result<Response<CreateVectorIndexResponse>, Status> {
        self.handle_create_vector_index(request).await
    }

    async fn get_vector_index(
        &self,
        request: Request<GetVectorIndexRequest>,
    ) -> Result<Response<GetVectorIndexResponse>, Status> {
        self.handle_get_vector_index(request).await
    }

    async fn list_vector_indexes(
        &self,
        request: Request<ListVectorIndexesRequest>,
    ) -> Result<Response<ListVectorIndexesResponse>, Status> {
        self.handle_list_vector_indexes(request).await
    }

    async fn drop_vector_index(
        &self,
        request: Request<DropVectorIndexRequest>,
    ) -> Result<Response<DropVectorIndexResponse>, Status> {
        self.handle_drop_vector_index(request).await
    }

    async fn search_vector(
        &self,
        request: Request<SearchVectorRequest>,
    ) -> Result<Response<SearchVectorResponse>, Status> {
        self.handle_search_vector(request).await
    }

    async fn get_version_history(
        &self,
        request: Request<VersionHistoryRequest>,
    ) -> Result<Response<VersionHistoryResponse>, Status> {
        self.handle_get_version_history(request).await
    }

    async fn get_schema_changes(
        &self,
        request: Request<SchemaChangesRequest>,
    ) -> Result<Response<SchemaChangesResponse>, Status> {
        self.handle_get_schema_changes(request).await
    }

    async fn detect_breaking_changes(
        &self,
        request: Request<BreakingChangesRequest>,
    ) -> Result<Response<BreakingChangesResponse>, Status> {
        self.handle_detect_breaking_changes(request).await
    }

    async fn migrate_plan(
        &self,
        request: Request<MigratePlanRequest>,
    ) -> Result<Response<MigratePlanResponse>, Status> {
        self.handle_migrate_plan(request).await
    }

    async fn migrate_execute(
        &self,
        request: Request<MigrateExecuteRequest>,
    ) -> Result<Response<MigrateExecuteResponse>, Status> {
        self.handle_migrate_execute(request).await
    }

    async fn migrate_rollback(
        &self,
        request: Request<MigrateRollbackRequest>,
    ) -> Result<Response<MigrateRollbackResponse>, Status> {
        self.handle_migrate_rollback(request).await
    }

    async fn stream_migration_progress(
        &self,
        request: Request<StreamMigrationProgressRequest>,
    ) -> Result<Response<Self::StreamMigrationProgressStream>, Status> {
        self.handle_stream_migration_progress(request).await
    }

    async fn dry_run_migration(
        &self,
        request: Request<DryRunMigrationRequest>,
    ) -> Result<Response<DryRunMigrationResponse>, Status> {
        self.handle_dry_run_migration(request).await
    }

    async fn get_migration_history(
        &self,
        request: Request<GetMigrationHistoryRequest>,
    ) -> Result<Response<GetMigrationHistoryResponse>, Status> {
        self.handle_get_migration_history(request).await
    }

    async fn get_migration_status(
        &self,
        request: Request<GetMigrationStatusRequest>,
    ) -> Result<Response<GetMigrationStatusResponse>, Status> {
        self.handle_get_migration_status(request).await
    }

    async fn open_cursor(
        &self,
        request: Request<OpenCursorRequest>,
    ) -> Result<Response<OpenCursorResponse>, Status> {
        self.handle_open_cursor(request).await
    }

    async fn fetch_cursor(
        &self,
        request: Request<FetchCursorRequest>,
    ) -> Result<Response<FetchCursorResponse>, Status> {
        self.handle_fetch_cursor(request).await
    }

    async fn close_cursor(
        &self,
        request: Request<CloseCursorRequest>,
    ) -> Result<Response<CloseCursorResponse>, Status> {
        self.handle_close_cursor(request).await
    }

    async fn list_transactions(
        &self,
        request: Request<ListTransactionsRequest>,
    ) -> Result<Response<ListTransactionsResponse>, Status> {
        self.handle_list_transactions(request).await
    }

    async fn kill_transaction(
        &self,
        request: Request<KillTransactionRequest>,
    ) -> Result<Response<KillTransactionResponse>, Status> {
        self.handle_kill_transaction(request).await
    }

    async fn get_transaction_metrics(
        &self,
        request: Request<GetTransactionMetricsRequest>,
    ) -> Result<Response<GetTransactionMetricsResponse>, Status> {
        self.handle_get_transaction_metrics(request).await
    }

    async fn get_sync_status(
        &self,
        request: Request<GetSyncStatusRequest>,
    ) -> Result<Response<GetSyncStatusResponse>, Status> {
        self.handle_get_sync_status(request).await
    }

    async fn retry_outbox(
        &self,
        request: Request<RetryOutboxRequest>,
    ) -> Result<Response<RetryOutboxResponse>, Status> {
        self.handle_retry_outbox(request).await
    }

    async fn get_outbox_diagnostics(
        &self,
        request: Request<GetOutboxDiagnosticsRequest>,
    ) -> Result<Response<GetOutboxDiagnosticsResponse>, Status> {
        self.handle_get_outbox_diagnostics(request).await
    }

    async fn list_dead_letters(
        &self,
        request: Request<ListDeadLettersRequest>,
    ) -> Result<Response<ListDeadLettersResponse>, Status> {
        self.handle_list_dead_letters(request).await
    }

    async fn requeue_dead_letters(
        &self,
        request: Request<RequeueDeadLettersRequest>,
    ) -> Result<Response<RequeueDeadLettersResponse>, Status> {
        self.handle_requeue_dead_letters(request).await
    }

    async fn get_retention_status(
        &self,
        request: Request<GetRetentionStatusRequest>,
    ) -> Result<Response<GetRetentionStatusResponse>, Status> {
        self.handle_get_retention_status(request).await
    }

    async fn run_retention(
        &self,
        request: Request<RunRetentionRequest>,
    ) -> Result<Response<RunRetentionResponse>, Status> {
        self.handle_run_retention(request).await
    }

    async fn import_data(
        &self,
        request: Request<ImportDataRequest>,
    ) -> Result<Response<ImportDataResponse>, Status> {
        self.handle_import_data(request).await
    }

    async fn get_import_status(
        &self,
        request: Request<GetImportStatusRequest>,
    ) -> Result<Response<GetImportStatusResponse>, Status> {
        self.handle_get_import_status(request).await
    }

    async fn export_data(
        &self,
        request: Request<ExportDataRequest>,
    ) -> Result<Response<ExportDataResponse>, Status> {
        self.handle_export_data(request).await
    }

    async fn rebuild_fulltext_index(
        &self,
        request: Request<RebuildFulltextIndexRequest>,
    ) -> Result<Response<RebuildFulltextIndexResponse>, Status> {
        self.handle_rebuild_fulltext_index(request).await
    }

    async fn get_fulltext_rebuild_status(
        &self,
        request: Request<GetFulltextRebuildStatusRequest>,
    ) -> Result<Response<GetFulltextRebuildStatusResponse>, Status> {
        self.handle_get_fulltext_rebuild_status(request).await
    }

    async fn clear_fulltext_index(
        &self,
        request: Request<ClearFulltextIndexRequest>,
    ) -> Result<Response<ClearFulltextIndexResponse>, Status> {
        self.handle_clear_fulltext_index(request).await
    }

    async fn list_inconsistent_fulltext(
        &self,
        request: Request<ListInconsistentFulltextRequest>,
    ) -> Result<Response<ListInconsistentFulltextResponse>, Status> {
        self.handle_list_inconsistent_fulltext(request).await
    }

    async fn explain_query(
        &self,
        request: Request<ExplainQueryRequest>,
    ) -> Result<Response<ExplainQueryResponse>, Status> {
        self.handle_explain_query(request).await
    }

    async fn execute_batch_query(
        &self,
        request: Request<ExecuteBatchQueryRequest>,
    ) -> Result<Response<ExecuteBatchQueryResponse>, Status> {
        self.handle_execute_batch_query(request).await
    }

    async fn get_config_key(
        &self,
        request: Request<GetConfigKeyRequest>,
    ) -> Result<Response<GetConfigKeyResponse>, Status> {
        self.handle_get_config_key(request).await
    }

    async fn bulk_update_config(
        &self,
        request: Request<BulkUpdateConfigRequest>,
    ) -> Result<Response<BulkUpdateConfigResponse>, Status> {
        self.handle_bulk_update_config(request).await
    }

    async fn delete_batch(
        &self,
        request: Request<DeleteBatchRequest>,
    ) -> Result<Response<DeleteBatchResponse>, Status> {
        self.handle_delete_batch(request).await
    }

    async fn scroll_vector(
        &self,
        request: Request<ScrollVectorRequest>,
    ) -> Result<Response<ScrollVectorResponse>, Status> {
        self.handle_scroll_vector(request).await
    }

    async fn get_vector_count(
        &self,
        request: Request<GetVectorCountRequest>,
    ) -> Result<Response<GetVectorCountResponse>, Status> {
        self.handle_get_vector_count(request).await
    }

    async fn rebuild_vector_index(
        &self,
        request: Request<RebuildVectorIndexRequest>,
    ) -> Result<Response<RebuildVectorIndexResponse>, Status> {
        self.handle_rebuild_vector_index(request).await
    }

    async fn get_vector_rebuild_status(
        &self,
        request: Request<GetVectorRebuildStatusRequest>,
    ) -> Result<Response<GetVectorRebuildStatusResponse>, Status> {
        self.handle_get_vector_rebuild_status(request).await
    }

    async fn clear_vector_index(
        &self,
        request: Request<ClearVectorIndexRequest>,
    ) -> Result<Response<ClearVectorIndexResponse>, Status> {
        self.handle_clear_vector_index(request).await
    }

    async fn get_overview_statistics(
        &self,
        request: Request<GetOverviewStatisticsRequest>,
    ) -> Result<Response<GetOverviewStatisticsResponse>, Status> {
        self.handle_get_overview_statistics(request).await
    }

    async fn get_search_statistics(
        &self,
        request: Request<GetSearchStatisticsRequest>,
    ) -> Result<Response<GetSearchStatisticsResponse>, Status> {
        self.handle_get_search_statistics(request).await
    }

    async fn get_migration_statistics(
        &self,
        request: Request<GetMigrationStatisticsRequest>,
    ) -> Result<Response<GetMigrationStatisticsResponse>, Status> {
        self.handle_get_migration_statistics(request).await
    }
}
