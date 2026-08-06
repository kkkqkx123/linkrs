//! Query Execution API – Core Layer
//!
//! Provides transport layer independent query execution

use crate::api::core::error::{CoreError, CoreResult};
use crate::api::core::types::{ExecutionMetadata, QueryRequest, QueryResult, Row};
use crate::core::metadata::SchemaManager;
use crate::core::StatsManager;
use crate::query::executor::streaming::pool::SharedScheduler;
use crate::query::executor::streaming::query_registry::QueryRegistry;
use crate::query::executor::streaming::StreamingQueryResult;
use crate::query::{OptimizerEngine, QueryPipelineManager};
use crate::storage::{AutoCommitBatchOps, QueryStorage, StorageClient, StorageOperationContext};
use crate::sync::SyncManager;
use crate::transaction::TransactionExecution;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;
#[cfg(feature = "qdrant")]
use vector_client::{EmbeddingService, VectorClientConfig, VectorManager};

/// Universal Query API – Core Layer
pub struct QueryApi<S: StorageClient + 'static> {
    pipeline_manager: QueryPipelineManager<S>,
}

impl<S: StorageClient + Clone + 'static> QueryApi<S> {
    /// Create a new QueryApi instance with external StatsManager
    pub fn new(storage: Arc<RwLock<S>>, stats_manager: Arc<StatsManager>) -> Self {
        let optimizer_engine = Arc::new(OptimizerEngine::default());
        Self {
            pipeline_manager: QueryPipelineManager::with_optimizer(
                storage,
                stats_manager,
                optimizer_engine,
            ),
        }
    }

    /// Create a new QueryApi instance with sync manager support
    pub fn with_sync_manager(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        sync_manager: Arc<SyncManager>,
    ) -> Self {
        let optimizer_engine = Arc::new(OptimizerEngine::default());
        Self {
            pipeline_manager: QueryPipelineManager::with_optimizer(
                storage,
                stats_manager,
                optimizer_engine,
            )
            .with_sync_manager(sync_manager),
        }
    }

    /// Create a new QueryApi instance with schema manager support
    pub fn with_schema_manager(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        schema_manager: Arc<SchemaManager>,
    ) -> Self {
        let optimizer_engine = Arc::new(OptimizerEngine::default());

        Self {
            pipeline_manager: QueryPipelineManager::with_optimizer(
                storage,
                stats_manager,
                optimizer_engine,
            )
            .with_schema_manager(schema_manager),
        }
    }

    /// Create a new QueryApi with both schema manager and sync manager support
    pub fn with_schema_and_sync_manager(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        schema_manager: Arc<SchemaManager>,
        sync_manager: Arc<SyncManager>,
    ) -> Self {
        let optimizer_engine = Arc::new(OptimizerEngine::default());

        Self {
            pipeline_manager: QueryPipelineManager::with_optimizer(
                storage,
                stats_manager,
                optimizer_engine,
            )
            .with_schema_manager(schema_manager)
            .with_sync_manager(sync_manager),
        }
    }

    /// Create a new QueryApi using an externally configured optimizer engine,
    /// so server-level settings (e.g. the `[parallel]` partitioning section)
    /// reach the query pipeline. The engine instance is shared by value.
    pub fn with_optimizer_engine(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        optimizer_engine: Arc<OptimizerEngine>,
        schema_manager: Option<Arc<SchemaManager>>,
    ) -> Self {
        let pipeline =
            QueryPipelineManager::with_optimizer(storage, stats_manager, optimizer_engine);
        let pipeline = match schema_manager {
            Some(sm) => pipeline.with_schema_manager(sm),
            None => pipeline,
        };
        Self {
            pipeline_manager: pipeline,
        }
    }

    /// Create a new QueryApi wired with an engine-level shared scheduler and
    /// a query registry, both created once at server startup and reused
    /// across all queries.
    pub fn with_shared_scheduler(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        optimizer_engine: Arc<OptimizerEngine>,
        shared_scheduler: Arc<SharedScheduler>,
        query_registry: Arc<QueryRegistry>,
    ) -> Self {
        Self {
            pipeline_manager: QueryPipelineManager::with_optimizer(
                storage,
                stats_manager,
                optimizer_engine,
            )
            .with_shared_scheduler(shared_scheduler)
            .with_query_registry(query_registry),
        }
    }

    /// Install a shared scheduler and query registry into an existing
    /// pipeline (used when the pipeline was built by another constructor).
    pub fn install_shared_scheduler(
        &mut self,
        shared_scheduler: Arc<SharedScheduler>,
        query_registry: Arc<QueryRegistry>,
    ) {
        self.pipeline_manager
            .set_shared_scheduler(Some(shared_scheduler));
        self.pipeline_manager
            .set_query_registry(Some(query_registry));
    }

    /// Access the shared scheduler instance held by the pipeline, if any.
    pub fn shared_scheduler(&self) -> Option<Arc<SharedScheduler>> {
        self.pipeline_manager.shared_scheduler()
    }

    /// Access the query registry instance held by the pipeline, if any.
    pub fn query_registry(&self) -> Option<Arc<QueryRegistry>> {
        self.pipeline_manager.query_registry()
    }

    /// Collect (or serve cached) optimizer statistics for a space.
    ///
    /// `force` bypasses the schema-version gate so an explicit ANALYZE always
    /// refreshes the statistics. Failures are returned as `Err`, never panic.
    pub fn collect_statistics(&self, space: &str, force: bool) -> Result<(), String> {
        self.pipeline_manager.collect_statistics(space, force)?;
        Ok(())
    }

    /// Query-plan-cache hit rate across all executed statements.
    pub fn plan_cache_hit_rate(&self) -> f64 {
        self.pipeline_manager.plan_cache_metrics().hit_rate()
    }

    /// Number of entries currently held in the query plan cache.
    pub fn plan_cache_len(&self) -> usize {
        self.pipeline_manager.plan_cache().len()
    }

    /// Create a new QueryApi instance with vector search support
    #[cfg(feature = "qdrant")]
    pub async fn with_vector_search(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        vector_config: VectorClientConfig,
        schema_manager: Option<Arc<SchemaManager>>,
    ) -> Result<Self, String> {
        let optimizer_engine = Arc::new(OptimizerEngine::default());

        // Extract embedding config before vector_manager consumes it
        let embedding_config = vector_config.embedding.clone();

        // Create vector manager
        let vector_manager = Arc::new(
            VectorManager::new(vector_config)
                .await
                .map_err(|e| format!("Failed to create vector manager: {}", e))?,
        );

        // Create optional embedding service
        let handle = tokio::runtime::Handle::current();
        let embedding_service =
            embedding_config.and_then(|ec| EmbeddingService::from_config(ec).ok().map(Arc::new));

        // Create vector coordinator (embedding service is optional)
        let vector_coordinator = Arc::new(crate::sync::vector_sync::VectorSyncCoordinator::new(
            vector_manager.clone(),
            embedding_service,
            handle,
        ));

        // Create pipeline manager with vector coordinator and optional schema manager
        let mut pipeline_manager =
            QueryPipelineManager::with_optimizer(storage, stats_manager, optimizer_engine);

        if let Some(sm) = schema_manager {
            pipeline_manager = pipeline_manager
                .with_schema_manager(sm)
                .with_vector_coordinator(vector_coordinator);
        } else {
            pipeline_manager = pipeline_manager.with_vector_coordinator(vector_coordinator);
        }

        Ok(Self { pipeline_manager })
    }

    /// Create a new QueryApi instance with an existing shared VectorManager
    #[cfg(feature = "qdrant")]
    pub async fn with_vector_manager(
        storage: Arc<RwLock<S>>,
        stats_manager: Arc<StatsManager>,
        vector_manager: Arc<VectorManager>,
        schema_manager: Option<Arc<SchemaManager>>,
    ) -> Result<Self, String> {
        let optimizer_engine = Arc::new(OptimizerEngine::default());

        // Create a VectorSyncCoordinator with the shared VectorManager (no embedding service for query-only use)
        let handle = tokio::runtime::Handle::current();
        let vector_coordinator = Arc::new(crate::sync::vector_sync::VectorSyncCoordinator::new(
            vector_manager,
            None,
            handle,
        ));

        // Create pipeline manager with vector coordinator and optional schema manager
        let mut pipeline_manager =
            QueryPipelineManager::with_optimizer(storage, stats_manager, optimizer_engine);

        if let Some(sm) = schema_manager {
            pipeline_manager = pipeline_manager
                .with_schema_manager(sm)
                .with_vector_coordinator(vector_coordinator);
        } else {
            pipeline_manager = pipeline_manager.with_vector_coordinator(vector_coordinator);
        }

        Ok(Self { pipeline_manager })
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

    pub fn execute_with_operation_context(
        &mut self,
        query: &str,
        ctx: QueryRequest,
        operation_context: Option<StorageOperationContext>,
    ) -> CoreResult<QueryResult> {
        self.execute_with_operation_context_and_storage(query, ctx, operation_context, None)
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
        let mut request_context = crate::query::QueryRequestContext::new(query.to_string());
        request_context.transaction_id = ctx.transaction_id.or_else(|| {
            operation_context
                .as_ref()
                .and_then(|context| context.transaction_id)
        });
        request_context.auto_commit = ctx.auto_commit;
        request_context.parameters = ctx.parameters.clone().unwrap_or_default();
        request_context.read_only = operation_context
            .as_ref()
            .is_some_and(|context| context.read_only);
        request_context.operation_context = operation_context;
        request_context.operation_storage = operation_storage
            .map(|storage| Arc::new(RwLock::new(storage)) as Arc<RwLock<dyn QueryStorage>>);
        let rctx = Arc::new(request_context);

        // Build space info from request context if space_id is provided
        let space_info = ctx.space_id.map(|id| {
            let space_name = ctx.space_name.clone().unwrap_or_default();
            let mut space_info = crate::core::types::SpaceInfo::new(space_name);
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
                return Err(CoreError::QueryExecutionFailed(error.to_string()));
            }
        };

        if ctx.auto_commit {
            if let Some(storage) = operation_finalizer.as_ref() {
                storage
                    .finalize_operation(true)
                    .map_err(|error| CoreError::StorageError(error.to_string()))?;
            }
        }

        // Conversion to structured results
        let mut result = Self::convert_to_query_result(execution_result)?;
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

    pub fn execute_stream_with_operation_context(
        &mut self,
        query: &str,
        ctx: QueryRequest,
        operation_context: Option<StorageOperationContext>,
    ) -> CoreResult<StreamingQueryResult> {
        self.execute_stream_with_operation_context_and_storage(query, ctx, operation_context, None)
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
        let mut request_context = crate::query::QueryRequestContext::new(query.to_string());
        request_context.transaction_id = ctx.transaction_id.or_else(|| {
            operation_context
                .as_ref()
                .and_then(|context| context.transaction_id)
        });
        request_context.auto_commit = ctx.auto_commit;
        request_context.parameters = ctx.parameters.clone().unwrap_or_default();
        request_context.read_only = operation_context
            .as_ref()
            .is_some_and(|context| context.read_only);
        request_context.operation_context = operation_context;
        request_context.operation_storage = operation_storage
            .map(|storage| Arc::new(RwLock::new(storage)) as Arc<RwLock<dyn QueryStorage>>);
        let rctx = Arc::new(request_context);

        let space_info = ctx.space_id.map(|id| {
            let space_name = ctx.space_name.clone().unwrap_or_default();
            let mut space_info = crate::core::types::SpaceInfo::new(space_name);
            space_info.space_id = id;
            space_info
        });

        let result = self
            .pipeline_manager
            .execute_query_stream_with_request_scope(query, rctx, space_info, ctx.transaction_id)
            .map_err(|e| CoreError::QueryExecutionFailed(e.to_string()))?;

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

    /// Execute a parameterized query
    pub fn execute_with_params(
        &mut self,
        query: &str,
        params: std::collections::HashMap<String, crate::core::Value>,
        ctx: QueryRequest,
    ) -> CoreResult<QueryResult> {
        // Create new QueryRequest with parameters
        let new_ctx = QueryRequest {
            space_id: ctx.space_id,
            space_name: ctx.space_name,
            auto_commit: ctx.auto_commit,
            transaction_id: ctx.transaction_id,
            parameters: Some(params),
        };
        self.execute(query, new_ctx)
    }

    /// Convert execution results to structured query results
    fn convert_to_query_result(
        execution: crate::query::executor::base::ExecutionResult,
    ) -> CoreResult<QueryResult> {
        match execution {
            crate::query::executor::base::ExecutionResult::DataSet { data, .. } => {
                // Processing the results of a dataset: The DataSet uses `col_names` instead of `columns`.
                let columns = data.col_names.clone();
                let mut rows = Vec::new();

                for row_data in &data.rows {
                    let mut row = Row::with_capacity(columns.len());
                    for (i, col) in columns.iter().enumerate() {
                        if let Some(value) = row_data.get(i) {
                            row.insert(col.clone(), value.clone());
                        }
                    }
                    rows.push(row);
                }

                let metadata = ExecutionMetadata {
                    execution_time_ms: 0,
                    rows_scanned: data.row_count() as u64,
                    rows_returned: data.row_count() as u64,
                    cache_hit: false,
                };

                Ok(QueryResult {
                    columns,
                    rows,
                    metadata,
                })
            }
            crate::query::executor::base::ExecutionResult::Success => {
                // Successful execution with no data
                Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    metadata: ExecutionMetadata::default(),
                })
            }
            crate::query::executor::base::ExecutionResult::Empty => {
                // Empty result
                Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    metadata: ExecutionMetadata::default(),
                })
            }
            crate::query::executor::base::ExecutionResult::SpaceSwitched(summary) => {
                // Space switched successfully
                let mut row = crate::api::core::types::Row::new();
                row.values.insert(
                    "space_name".to_string(),
                    crate::core::Value::string(summary.name.clone()),
                );
                row.values.insert(
                    "space_id".to_string(),
                    crate::core::Value::BigInt(summary.id as i64),
                );
                row.values.insert(
                    "vid_type".to_string(),
                    crate::core::Value::string(summary.vid_type.to_string()),
                );
                Ok(QueryResult {
                    columns: vec![
                        "space_name".to_string(),
                        "space_id".to_string(),
                        "vid_type".to_string(),
                    ],
                    rows: vec![row],
                    metadata: ExecutionMetadata::default(),
                })
            }
            crate::query::executor::base::ExecutionResult::Error(msg) => {
                // Error case - should be handled before this function
                Err(CoreError::Internal(msg))
            }
        }
    }
}

impl<S> QueryApi<S>
where
    S: StorageClient + Clone + AutoCommitBatchOps + 'static,
{
    /// Execute a batch of auto-commit DML statements inside a single
    /// [`AutoCommitBatchWindow`](crate::storage::AutoCommitBatchWindow) (P4/P6).
    ///
    /// Acquires the auto-commit write gate and registers MVCC snapshots once
    /// for the whole batch instead of once per statement. Each statement still
    /// executes independently: it allocates its own write timestamp /
    /// transaction id / undo log, commits on success, and rolls back its own
    /// partial writes on failure. Intended for auto-commit DML loads (e.g.
    /// `load_gql_file`); mixed-in read statements are harmless but do not
    /// share any batching benefit. **DDL statements must not be mixed in**:
    /// they are not exercised through the auto-commit window and may leave
    /// the window inconsistent.
    ///
    /// Errors do not abort the batch: every statement runs to completion and
    /// the returned vector holds one `Ok`/`Err` per input statement, in order.
    /// The window is always finalized before returning.
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

    /// Number of P6 Level 2 same-shape DML plan-memo hits since pipeline
    /// creation (observation for the batch-load regression test).
    pub fn dml_plan_memo_hits(&self) -> u64 {
        self.pipeline_manager
            .last_dml_plan_hits
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}
