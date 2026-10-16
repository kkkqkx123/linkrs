//! Query Execution API – Core Layer
//!
//! Provides transport layer independent query execution.
//!
//! The original single `query_api.rs` (≈802 lines) mixed the execution core
//! with ~20 `with_*` builder / configuration methods. The builders now live in
//! the sibling `query_api_builder` module (declared alongside this file in
//! `api_core.rs`); this file keeps the type, the constructor, the execution
//! core, the batch helpers, and the unit tests.

use crate::api_core::error::{CoreError, CoreResult};
use crate::api_core::types::{ExecutionMetadata, QueryRequest, QueryResult};
use linkrs_metrics::StatsManager;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::executor::streaming::StreamingQueryResult;
use linkrs_query::{OptimizerEngine, QueryPipelineManager};
use linkrs_storage::{
    AutoCommitBatchOps, AutoCommitGroupOps, QueryStorage, StorageClient, StorageOperationContext,
};
use linkrs_transaction::TransactionExecution;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

/// Universal Query API – Core Layer
pub struct QueryApi<S: StorageClient + 'static> {
    pub(crate) pipeline_manager: QueryPipelineManager<S>,
}

impl<S: StorageClient + Clone + 'static> QueryApi<S> {
    /// Shared assembly core: every public constructor delegates here so the
    /// optimizer engine wiring cannot drift between constructors.
    pub(crate) fn pipeline_with_engine(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        optimizer_engine: Arc<OptimizerEngine>,
    ) -> QueryPipelineManager<S> {
        QueryPipelineManager::with_optimizer(storage, stats_manager, optimizer_engine)
    }

    /// Create a new QueryApi instance with external StatsManager
    pub fn new(storage: Arc<RwLock<S>>, stats_manager: Arc<StatsManager>) -> Self {
        let optimizer_engine = Arc::new(OptimizerEngine::default());
        Self {
            pipeline_manager: Self::pipeline_with_engine(storage, stats_manager, optimizer_engine),
        }
    }

    /// Execute a query with an explicit transaction execution binding.
    ///
    /// The `execution` parameter carries the full transaction identity
    /// (ID, timestamps, mode, owner) from `TransactionManager`.
    /// This is the preferred entry point for all transactional DML.
    pub fn execute_with_execution(
        &mut self,
        query: &str,
        ctx: QueryRequest,
        execution: &TransactionExecution,
    ) -> CoreResult<QueryResult> {
        let op_ctx = StorageOperationContext::transaction_with_timestamps(
            execution.transaction_id(),
            execution.read_timestamp(),
            execution.write_timestamp(),
            execution.read_only(),
            execution.auto_commit(),
        );
        let op_ctx = execution
            .mutation_recorder()
            .map_or(op_ctx.clone(), |recorder| {
                op_ctx.with_mutation_recorder(recorder)
            });
        let mut ctx = ctx;
        ctx.transaction_id = Some(execution.transaction_id());
        ctx.auto_commit = execution.auto_commit();
        ctx.isolation_level = Some(execution.isolation_level());
        self.execute_with_operation_context_and_storage(query, ctx, Some(op_ctx), None)
    }

    /// Execute a query with the given query request
    ///
    /// # Parameters
    /// `query`: The query statement
    /// - `ctx`: query request
    ///
    /// # Return
    /// Structured Search Results
    pub fn execute(&mut self, query: &str, ctx: QueryRequest) -> CoreResult<QueryResult> {
        self.execute_with_operation_context_and_storage(query, ctx, None, None)
    }

    pub fn execute_with_operation_storage(
        &mut self,
        query: &str,
        ctx: QueryRequest,
        storage: S,
    ) -> CoreResult<QueryResult> {
        let operation_context = storage.operation_context().as_deref().cloned();
        self.execute_with_operation_context_and_storage(
            query,
            ctx,
            operation_context,
            Some(storage),
        )
    }

    fn execute_with_operation_context_and_storage(
        &mut self,
        query: &str,
        ctx: QueryRequest,
        operation_context: Option<StorageOperationContext>,
        operation_storage: Option<S>,
    ) -> CoreResult<QueryResult> {
        let start_time = Instant::now();
        let operation_finalizer = operation_storage.clone();

        // Constructing a QueryRequestContext
        let mut request_context = linkrs_query::QueryRequestContext::new(query.to_string());
        request_context.query_id = ctx.query_id;
        request_context.transaction_id = ctx.transaction_id.or_else(|| {
            operation_context
                .as_ref()
                .and_then(|context| context.transaction_id)
        });
        request_context.auto_commit = ctx.auto_commit;
        request_context.parameters = ctx.parameters.clone().unwrap_or_default();
        request_context.session_variables = ctx.session_variables.clone().unwrap_or_default();
        request_context.parsed_statement = ctx.parsed_statement.clone();
        request_context.read_only = operation_context
            .as_ref()
            .is_some_and(|context| context.read_only);
        request_context.isolation_level = ctx.isolation_level;
        request_context.ryw_config = ctx.consistency.read_your_writes().copied();
        request_context.operation_context = operation_context;
        request_context.operation_storage = operation_storage
            .map(|storage| Arc::new(RwLock::new(storage)) as Arc<RwLock<dyn QueryStorage>>);
        let rctx = Arc::new(request_context);

        // Build space info from request context if space_id is provided
        let space_info = ctx.space_id.map(|id| {
            let space_name = ctx.space_name.clone().unwrap_or_default();
            let mut space_info = linkrs_core::types::SpaceInfo::new(space_name);
            space_info.space_id = id;
            space_info
        });

        // Execute the query (using the new execute_query_with_request method).
        let execution_result = match self.pipeline_manager.execute_query_with_request_scope(
            query,
            rctx,
            space_info,
            ctx.transaction_id,
        ) {
            Ok(result) => result,
            Err(error) => {
                if ctx.auto_commit {
                    if let Some(storage) = operation_finalizer.as_ref() {
                        storage
                            .finalize_operation(false)
                            .map_err(|cleanup| CoreError::StorageError(cleanup.to_string()))?;
                    }
                }
                return Err(CoreError::from(error));
            }
        };

        if ctx.auto_commit {
            if let Some(storage) = operation_finalizer.as_ref() {
                storage
                    .finalize_operation(true)
                    .map_err(|error| CoreError::StorageError(error.to_string()))?;
            }
        }

        // Wrap the engine execution result with API-layer metadata.
        let mut result = Self::attach_metadata(execution_result)?;
        result.metadata.execution_time_ms = start_time.elapsed().as_millis() as u64;

        Ok(result)
    }

    /// Execute a query with an explicit transaction execution binding (streaming).
    pub fn execute_stream_with_execution(
        &mut self,
        query: &str,
        ctx: QueryRequest,
        execution: &TransactionExecution,
    ) -> CoreResult<StreamingQueryResult> {
        let op_ctx = StorageOperationContext::transaction_with_timestamps(
            execution.transaction_id(),
            execution.read_timestamp(),
            execution.write_timestamp(),
            execution.read_only(),
            execution.auto_commit(),
        );
        let op_ctx = execution
            .mutation_recorder()
            .map_or(op_ctx.clone(), |recorder| {
                op_ctx.with_mutation_recorder(recorder)
            });
        let mut ctx = ctx;
        ctx.transaction_id = Some(execution.transaction_id());
        ctx.auto_commit = execution.auto_commit();
        ctx.isolation_level = Some(execution.isolation_level());
        self.execute_stream_with_operation_context_and_storage(query, ctx, Some(op_ctx), None)
    }

    /// Execute a query and return a [`StreamingQueryResult`] for chunk-at-a-time consumption.
    ///
    /// Unlike [`execute`] which materialises the full result set, this method
    /// returns a thread-safe streaming handle that lets the caller pull chunks
    /// one at a time.  Useful for SSE / gRPC streaming endpoints.
    pub fn execute_stream(
        &mut self,
        query: &str,
        ctx: QueryRequest,
    ) -> CoreResult<StreamingQueryResult> {
        self.execute_stream_with_operation_context_and_storage(query, ctx, None, None)
    }

    pub fn execute_stream_with_operation_storage(
        &mut self,
        query: &str,
        ctx: QueryRequest,
        storage: S,
    ) -> CoreResult<StreamingQueryResult> {
        let operation_context = storage.operation_context().as_deref().cloned();
        self.execute_stream_with_operation_context_and_storage(
            query,
            ctx,
            operation_context,
            Some(storage),
        )
    }

    fn execute_stream_with_operation_context_and_storage(
        &mut self,
        query: &str,
        ctx: QueryRequest,
        operation_context: Option<StorageOperationContext>,
        operation_storage: Option<S>,
    ) -> CoreResult<StreamingQueryResult> {
        let operation_owned = operation_storage.is_some() && ctx.auto_commit;
        let mut request_context = linkrs_query::QueryRequestContext::new(query.to_string());
        request_context.query_id = ctx.query_id;
        request_context.transaction_id = ctx.transaction_id.or_else(|| {
            operation_context
                .as_ref()
                .and_then(|context| context.transaction_id)
        });
        request_context.auto_commit = ctx.auto_commit;
        request_context.parameters = ctx.parameters.clone().unwrap_or_default();
        request_context.session_variables = ctx.session_variables.clone().unwrap_or_default();
        request_context.parsed_statement = ctx.parsed_statement.clone();
        request_context.read_only = operation_context
            .as_ref()
            .is_some_and(|context| context.read_only);
        request_context.isolation_level = ctx.isolation_level;
        request_context.ryw_config = ctx.consistency.read_your_writes().copied();
        request_context.operation_context = operation_context;
        request_context.operation_storage = operation_storage
            .map(|storage| Arc::new(RwLock::new(storage)) as Arc<RwLock<dyn QueryStorage>>);
        let rctx = Arc::new(request_context);

        let space_info = ctx.space_id.map(|id| {
            let space_name = ctx.space_name.clone().unwrap_or_default();
            let mut space_info = linkrs_core::types::SpaceInfo::new(space_name);
            space_info.space_id = id;
            space_info
        });

        let result = self
            .pipeline_manager
            .execute_query_stream_with_request_scope(query, rctx, space_info, ctx.transaction_id)
            .map_err(CoreError::from)?;

        if operation_owned {
            if let Some(storage) = result.runtime().storage.clone() {
                let commit_storage = storage.clone();
                let abort_storage = storage;
                result.set_transaction_finalizer_with_result(
                    Box::new(move || {
                        commit_storage
                            .write()
                            .finalize_operation(true)
                            .map_err(|error| error.to_string())
                    }),
                    Box::new(move || {
                        abort_storage
                            .write()
                            .finalize_operation(false)
                            .map_err(|error| error.to_string())
                    }),
                );
            }
        }
        Ok(result)
    }

    /// Attach API-layer metadata to an engine execution result.
    ///
    /// The engine result is carried through unchanged (no row conversion):
    /// the API core layer adds only timing / count metadata. `ExecutionResult::Error`
    /// is surfaced as an internal error, all other variants pass through.
    /// Scanned rows stay zero here because the true scan volume is owned by
    /// executor statistics, not by the returned row count.
    fn attach_metadata(execution: ExecutionResult) -> CoreResult<QueryResult> {
        let rows_returned = match &execution {
            ExecutionResult::DataSet { data, .. } => data.row_count(),
            _ => 0,
        };
        match execution {
            ExecutionResult::Error(msg) => Err(CoreError::Internal(msg)),
            other => Ok(QueryResult::new(
                other,
                ExecutionMetadata {
                    execution_time_ms: 0,
                    rows_scanned: 0,
                    rows_returned: rows_returned as u64,
                    cache_hit: false,
                },
            )),
        }
    }
}

impl<S> QueryApi<S>
where
    S: StorageClient + Clone + AutoCommitBatchOps + 'static,
{
    pub fn execute_batch(
        &mut self,
        queries: &[String],
        ctx: QueryRequest,
    ) -> Vec<Result<QueryResult, CoreError>> {
        let mut results = Vec::with_capacity(queries.len());
        let base_storage = match self.pipeline_manager.storage() {
            Some(storage) => storage,
            None => {
                for _ in queries {
                    results.push(Err(CoreError::Internal("No storage binding".to_string())));
                }
                return results;
            }
        };

        let window = match base_storage.read().begin_auto_commit_batch() {
            Ok(window) => window,
            Err(error) => {
                for _ in queries {
                    results.push(Err(CoreError::StorageError(error.to_string())));
                }
                return results;
            }
        };

        for query in queries {
            let stmt_storage = match base_storage.read().bind_auto_commit_statement(&window) {
                Ok(storage) => storage,
                Err(error) => {
                    results.push(Err(CoreError::StorageError(error.to_string())));
                    continue;
                }
            };
            let mut batch_ctx = ctx.clone();
            batch_ctx.auto_commit = true;
            batch_ctx.transaction_id = None;
            results.push(self.execute_with_operation_storage(query, batch_ctx, stmt_storage));
        }

        if let Err(error) = base_storage.read().finalize_auto_commit_batch(&window) {
            results.push(Err(CoreError::StorageError(error.to_string())));
        }
        results
    }

    pub fn dml_plan_memo_hits(&self) -> u64 {
        self.pipeline_manager
            .last_dml_plan_hits
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn dml_template_ast_parse_count(&self) -> u64 {
        self.pipeline_manager.dml_template_ast_parse_count()
    }

    pub fn dml_template_ast_hit_count(&self) -> u64 {
        self.pipeline_manager.dml_template_ast_hit_count()
    }
}

impl<S> QueryApi<S>
where
    S: StorageClient + Clone + AutoCommitBatchOps + AutoCommitGroupOps + 'static,
{
    /// Execute a batch of auto-commit DML statements in group-commit windows.
    /// `group_size` is clamped to >= 1; each consecutive group of statements
    /// shares one write timestamp, one WAL fsync, and one commit point.
    /// Statement results are returned per statement, in order, exactly like
    /// `execute_batch`; failed statements roll back only their own writes.
    /// DDL / transaction statements / USE must not be mixed in (same contract
    /// as `execute_batch`).
    pub fn execute_batch_grouped(
        &mut self,
        queries: &[String],
        ctx: QueryRequest,
        group_size: usize,
    ) -> Vec<Result<QueryResult, CoreError>> {
        let group_size = group_size.max(1);
        let mut results = Vec::with_capacity(queries.len());
        let base_storage = match self.pipeline_manager.storage() {
            Some(storage) => storage,
            None => {
                for _ in queries {
                    results.push(Err(CoreError::Internal("No storage binding".to_string())));
                }
                return results;
            }
        };

        for chunk in queries.chunks(group_size) {
            let window = match base_storage.read().begin_auto_commit_group() {
                Ok(window) => window,
                Err(error) => {
                    for _ in chunk {
                        results.push(Err(CoreError::StorageError(error.to_string())));
                    }
                    continue;
                }
            };

            for query in chunk {
                let stmt_storage = match base_storage.read().bind_auto_commit_statement(&window) {
                    Ok(storage) => storage,
                    Err(error) => {
                        results.push(Err(CoreError::StorageError(error.to_string())));
                        continue;
                    }
                };
                let mut batch_ctx = ctx.clone();
                batch_ctx.auto_commit = true;
                batch_ctx.transaction_id = None;
                results.push(self.execute_with_operation_storage(query, batch_ctx, stmt_storage));
            }

            if let Err(error) = base_storage.read().finalize_auto_commit_group(&window) {
                results.push(Err(CoreError::StorageError(error.to_string())));
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linkrs_storage::MockStorage;

    fn create_mock_storage() -> Arc<RwLock<MockStorage>> {
        Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ))
    }

    #[test]
    fn statistics_settings_default_to_pipeline_fallbacks() {
        let api = QueryApi::new(create_mock_storage(), Arc::new(StatsManager::new()));
        assert_eq!(
            api.statistics_sample_limit(),
            linkrs_query::QueryPipelineManager::<MockStorage>::DEFAULT_STATISTICS_SAMPLE_LIMIT
        );
        assert_eq!(
            api.statistics_min_epoch_delta(),
            linkrs_query::QueryPipelineManager::<MockStorage>::DEFAULT_STATISTICS_MIN_EPOCH_DELTA
        );
    }

    #[test]
    fn statistics_settings_override_reaches_pipeline() {
        let api = QueryApi::new(create_mock_storage(), Arc::new(StatsManager::new()))
            .with_statistics_settings(500, 10);
        assert_eq!(api.statistics_sample_limit(), 500);
        assert_eq!(api.statistics_min_epoch_delta(), 10);
    }

    #[test]
    fn statistics_sample_limit_clamps_to_one() {
        let api = QueryApi::new(create_mock_storage(), Arc::new(StatsManager::new()))
            .with_statistics_settings(0, 0);
        assert_eq!(api.statistics_sample_limit(), 1);
        assert_eq!(api.statistics_min_epoch_delta(), 0);
    }

    #[test]
    fn cost_config_defaults_to_engine_default() {
        let api = QueryApi::new(create_mock_storage(), Arc::new(StatsManager::new()));
        assert_eq!(
            api.cost_config().random_page_cost,
            linkrs_query::optimizer::CostModelConfig::default().random_page_cost
        );
    }

    #[test]
    fn cost_profile_memory_runtime_reaches_engine() {
        let api = QueryApi::new(create_mock_storage(), Arc::new(StatsManager::new()))
            .with_cost_profile(StorageCostProfile::Auto, &RuntimeConfig::memory());
        assert_eq!(
            api.cost_config().random_page_cost,
            linkrs_query::optimizer::CostModelConfig::for_in_memory().random_page_cost
        );
    }

    #[test]
    fn space_overrides_reach_engine_calculators() {
        use std::collections::HashMap;

        let mut profiles = HashMap::new();
        profiles.insert("cold".into(), StorageCostProfile::Hdd);
        let api = QueryApi::new(create_mock_storage(), Arc::new(StatsManager::new()))
            .with_cost_profile(StorageCostProfile::Ssd, &RuntimeConfig::memory())
            .with_space_cost_profiles(&profiles, &RuntimeConfig::memory());
        // Memory runtime short-circuits the Hdd override to in-memory.
        assert_eq!(
            api.cost_config().random_page_cost,
            linkrs_query::optimizer::CostModelConfig::for_in_memory().random_page_cost
        );
        let engine = api.pipeline_manager.optimizer_engine();
        assert_eq!(
            engine
                .cost_calculator_for(Some("cold"))
                .config()
                .random_page_cost,
            linkrs_query::optimizer::CostModelConfig::for_in_memory().random_page_cost
        );
    }

    #[test]
    fn cost_config_rebuild_preserves_space_overrides() {
        use std::collections::HashMap;

        let mut profiles = HashMap::new();
        profiles.insert("cold".into(), StorageCostProfile::Hdd);
        let runtime = RuntimeConfig::file("/tmp/linkrs-test-data");
        let api = QueryApi::new(create_mock_storage(), Arc::new(StatsManager::new()))
            .with_space_cost_profiles(&profiles, &runtime)
            .with_cost_config(linkrs_query::optimizer::CostModelConfig::for_ssd());
        let engine = api.pipeline_manager.optimizer_engine();
        // Undetectable dir falls back to Ssd; the Hdd space override survives
        // the engine rebuild and still prices random pages at 4.0.
        assert_eq!(
            engine
                .cost_calculator_for(Some("cold"))
                .config()
                .random_page_cost,
            linkrs_query::optimizer::CostModelConfig::for_hdd().random_page_cost
        );
        assert_eq!(
            engine
                .cost_calculator_for(Some("hot"))
                .config()
                .random_page_cost,
            linkrs_query::optimizer::CostModelConfig::for_ssd().random_page_cost
        );
    }
    #[test]
    fn memory_runtime_short_circuits_explicit_preset() {
        let api = QueryApi::new(create_mock_storage(), Arc::new(StatsManager::new()))
            .with_cost_profile(StorageCostProfile::Hdd, &RuntimeConfig::memory());
        assert_eq!(
            api.cost_config().random_page_cost,
            linkrs_query::optimizer::CostModelConfig::for_in_memory().random_page_cost
        );
    }
}
