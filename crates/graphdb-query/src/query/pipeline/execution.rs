use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::types::SpaceInfo;
use crate::core::types::TransactionId;
use crate::core::{
    ErrorInfo, ErrorType, MetricType, QueryMetrics, QueryPhase, QueryProfile, StatsManager,
};
use crate::query::executor::base::{ExecutionContext, ExecutionResult};
use crate::query::executor::streaming::instance::{
    QueryBindings, QueryExecutionInstance, ResultSink,
};
use crate::query::executor::streaming::plan::PhysicalPlan;
use crate::query::executor::streaming::transaction_scope::TransactionScope;
use crate::query::executor::streaming::StreamingQueryResult;
use crate::query::QueryContext;
use crate::query::QueryRequestContext;
use crate::storage::QueryStorage;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub fn execute_query(&mut self, query_text: &str) -> DBResult<ExecutionResult> {
        self.execute_query_with_space(query_text, None)
    }

    /// Convenience entry: parse, bind auto-commit storage for DML, compile,
    /// execute, and finalize in one call.  Used by tests and the embedded API.
    ///
    /// Finalization of the auto-bound operation storage is performed by
    /// [`execute_prepared_materialized`].
    pub fn execute_query_with_space(
        &mut self,
        query_text: &str,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<ExecutionResult> {
        let request = self.prepare_request_with_auto_commit(query_text, space_info)?;
        match self.execute_prepared(&request, None, ResultSink::Materialize)? {
            super::prepared::PreparedOutcome::Materialized(result) => Ok(result),
            _ => unreachable!("materialize sink cannot stream"),
        }
    }

    pub fn execute_query_stream_with_request(
        &mut self,
        query_text: &str,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<StreamingQueryResult> {
        self.execute_query_stream_with_request_scope(query_text, rctx, space_info, None)
    }

    pub fn execute_query_stream_with_request_scope(
        &mut self,
        query_text: &str,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<SpaceInfo>,
        transaction_id: Option<TransactionId>,
    ) -> DBResult<StreamingQueryResult> {
        let request = self.prepare_request(query_text, rctx, space_info)?;
        match self.execute_prepared(&request, transaction_id, ResultSink::Stream)? {
            super::prepared::PreparedOutcome::Stream(stream) => Ok(stream),
            _ => unreachable!("stream sink cannot materialize"),
        }
    }

    pub fn execute_query_with_request(
        &mut self,
        query_text: &str,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<ExecutionResult> {
        self.execute_query_with_request_scope(query_text, rctx, space_info, None)
    }

    pub fn execute_query_with_request_scope(
        &mut self,
        query_text: &str,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<SpaceInfo>,
        transaction_id: Option<TransactionId>,
    ) -> DBResult<ExecutionResult> {
        let request = self.prepare_request(query_text, rctx, space_info)?;
        match self.execute_prepared(&request, transaction_id, ResultSink::Materialize)? {
            super::prepared::PreparedOutcome::Materialized(result) => Ok(result),
            _ => unreachable!("materialize sink cannot stream"),
        }
    }

    pub fn execute_query_with_metrics(
        &mut self,
        query_text: &str,
    ) -> DBResult<(ExecutionResult, QueryMetrics)> {
        self.execute_query_with_session(query_text, 0)
            .map(|(result, metrics, _)| (result, metrics))
    }

    pub fn execute_query_with_session(
        &mut self,
        query_text: &str,
        session_id: i64,
    ) -> DBResult<(ExecutionResult, QueryMetrics, QueryProfile)> {
        self.execute_query_with_profile(query_text, session_id)
    }

    pub fn execute_query_with_profile(
        &mut self,
        query_text: &str,
        session_id: i64,
    ) -> DBResult<(ExecutionResult, QueryMetrics, QueryProfile)> {
        self.stats_manager.add_value(MetricType::NumQueries);
        self.stats_manager.add_value(MetricType::NumActiveQueries);

        struct ActiveQueryGuard {
            stats_manager: Arc<StatsManager>,
        }
        impl Drop for ActiveQueryGuard {
            fn drop(&mut self) {
                self.stats_manager.dec_value(MetricType::NumActiveQueries);
            }
        }
        let _guard = ActiveQueryGuard {
            stats_manager: self.stats_manager.clone(),
        };

        let total_start = Instant::now();
        let mut metrics = QueryMetrics::new();
        let mut profile = QueryProfile::new(session_id, query_text.to_string());

        // Use prepared lifecycle for parse + validate
        let prepare_start = Instant::now();
        let request = match self.prepare_request_with_auto_commit(query_text, None) {
            Ok(req) => {
                profile.stages.parse_us = prepare_start.elapsed().as_micros() as u64;
                profile.stages.validate_us = 0;
                metrics.record_parse_time(prepare_start.elapsed());
                req
            }
            Err(e) => {
                profile.stages.parse_us = prepare_start.elapsed().as_micros() as u64;
                let error_info =
                    ErrorInfo::new(ErrorType::ParseError, QueryPhase::Parse, e.to_string());
                profile.mark_failed_with_info(error_info.clone());
                profile.total_duration_us = total_start.elapsed().as_micros() as u64;
                self.stats_manager
                    .record_failed_query(profile.clone(), error_info);
                return Err(e);
            }
        };

        self.record_query_type_counter(&request.stmt);

        // Dedicated compile + execute: `execute_prepared` runs the generated
        // diagnostic/analyze/DDL paths internally and uses the shared plan
        // cache.  Compilation is folded into the execute phase for profiling
        // (optimization happens inside the cache-compile step).
        let execute_start = Instant::now();
        let result = match self.execute_prepared(&request, None, ResultSink::Materialize) {
            Ok(super::prepared::PreparedOutcome::Materialized(result)) => result,
            Ok(_) => unreachable!("materialize sink cannot stream"),
            Err(e) => {
                profile.stages.execute_us = execute_start.elapsed().as_micros() as u64;
                let error_info = ErrorInfo::new(
                    ErrorType::ExecutionError,
                    QueryPhase::Execute,
                    e.to_string(),
                );
                profile.mark_failed_with_info(error_info.clone());
                profile.total_duration_us = total_start.elapsed().as_micros() as u64;
                self.stats_manager
                    .record_failed_query(profile.clone(), error_info);
                return Err(e);
            }
        };

        profile.stages.execute_us = execute_start.elapsed().as_micros() as u64;
        profile.result_count = result.count();
        metrics.set_result_row_count(result.count());
        metrics.record_execute_time(execute_start.elapsed());

        profile.total_duration_us = total_start.elapsed().as_micros() as u64;
        metrics.record_total_time(total_start.elapsed());

        self.stats_manager.record_query_metrics(&metrics);
        self.stats_manager.record_query_profile(profile.clone());

        Ok((result, metrics, profile))
    }

    pub(crate) fn execute_compiled_with_scope(
        &self,
        physical_plan: Arc<PhysicalPlan>,
        query_context: Arc<QueryContext>,
        sink: ResultSink,
        transaction_scope: TransactionScope,
    ) -> DBResult<ExecutionResult> {
        if matches!(sink, ResultSink::Stream) {
            return Err(DBError::from(QueryError::execution(
                "Use execute_compiled_stream_with_scope for streaming sink".to_string(),
            )));
        }

        let exec_ctx = self.build_execution_context(&query_context);
        let is_command_scope = matches!(transaction_scope, TransactionScope::CommandScope);
        let mut bindings = QueryBindings::from_context(&exec_ctx, transaction_scope);
        bindings.query_id = exec_ctx.query_id;
        bindings.query_text = Some(query_context.request_context().query.clone());
        bindings.session_id = query_context
            .request_context()
            .session_id
            .map(|id| id.to_string());
        bindings.user_name = query_context.request_context().user_name.clone();

        let mut instance = QueryExecutionInstance::instantiate_plan(
            physical_plan,
            bindings,
            sink,
            self.query_registry.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        if is_command_scope {
            let mut ctrl_guard = self.session_controller.write();
            let controller = if ctrl_guard.as_ref().is_some_and(|c| c.is_active()) {
                ctrl_guard.clone().unwrap()
            } else {
                let ctrl = Arc::new(
                    crate::query::executor::streaming::SessionTransactionController::new(),
                );
                if let Some(txn_id) = query_context.request_context().transaction_id {
                    let read_write = !query_context.request_context().read_only;
                    ctrl.begin_tracking(txn_id, read_write)
                        .map_err(|error| DBError::from(QueryError::execution(error.to_string())))?;
                }
                *ctrl_guard = Some(ctrl.clone());
                ctrl
            };
            drop(ctrl_guard);
            instance.runtime().set_session_controller(controller);
        }

        match sink {
            ResultSink::Materialize => instance
                .execute()
                .map_err(|e| DBError::from(QueryError::execution(e.to_string()))),
            ResultSink::Discard => {
                instance
                    .execute_discard()
                    .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;
                Ok(ExecutionResult::Empty)
            }
            ResultSink::Stream => unreachable!("stream sink is rejected before instantiation"),
        }
    }

    pub(crate) fn execute_compiled_stream_with_scope(
        &self,
        physical_plan: Arc<PhysicalPlan>,
        query_context: Arc<QueryContext>,
        transaction_scope: TransactionScope,
    ) -> DBResult<StreamingQueryResult> {
        let exec_ctx = self.build_execution_context(&query_context);
        let mut bindings = QueryBindings::from_context(&exec_ctx, transaction_scope);
        bindings.query_id = exec_ctx.query_id;
        bindings.query_text = Some(query_context.request_context().query.clone());
        bindings.session_id = query_context
            .request_context()
            .session_id
            .map(|id| id.to_string());
        bindings.user_name = query_context.request_context().user_name.clone();

        let instance = QueryExecutionInstance::instantiate_plan(
            physical_plan,
            bindings,
            ResultSink::Stream,
            self.query_registry.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        instance
            .into_stream()
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))
    }

    pub(crate) fn build_execution_context(&self, query_context: &QueryContext) -> ExecutionContext {
        use std::collections::HashMap;

        let params: HashMap<String, crate::core::Value> =
            query_context.request_context().parameters.clone();
        let session_variables: HashMap<String, crate::core::Value> =
            query_context.request_context().session_variables.clone();

        let mut context = ExecutionContext {
            max_workers: self
                .optimizer_engine
                .partitioning_config()
                .max_workers
                .max(1),
            max_buffered_chunks: self
                .optimizer_engine
                .partitioning_config()
                .max_buffered_chunks
                .max(1),
            parameters: Arc::new(params),
            session_variables: Arc::new(session_variables),
            ..ExecutionContext::default()
        };
        context.shared_scheduler = self.shared_scheduler.clone();
        if let Some(ref storage) = self.storage {
            let dyn_storage: Arc<RwLock<dyn QueryStorage>> = if let Some(operation_storage) =
                query_context.request_context().operation_storage.clone()
            {
                operation_storage
            } else if let Some(operation) =
                query_context.request_context().operation_context.clone()
            {
                let bound_storage = storage.read().bind_operation_context(operation);
                Arc::new(RwLock::new(bound_storage))
            } else {
                storage.clone()
            };
            // P2: surface the snapshot pinned by the per-query bound storage.
            // Read-only statements bind a fixed read timestamp; DML binds its
            // write timestamp. Unbound raw storage reports None.
            context.bound_snapshot = dyn_storage.read().snapshot_handle();
            context.storage = Some(dyn_storage);
        }
        #[cfg(feature = "fulltext-search")]
        {
            context.fulltext_manager = self.fulltext_manager.clone();
        }
        #[cfg(feature = "vector")]
        {
            context.vector_coordinator = self.vector_coordinator.clone();
        }
        if let Some(ref space_name) = query_context.space_name() {
            context.space_name = Some(space_name.clone());
        }
        if let Some(query_id) = query_context.request_context().query_id {
            context.query_id = query_id;
        }
        context.cancel_token = Some(query_context.cancel_token());
        if query_context.has_arena() {
            context.arena = Some(Arc::new(
                parking_lot::Mutex::new(crate::utils::Arena::new()),
            ));
        }
        // Stats feedback loop (phase 1): share the engine's feedback history
        // with every execution so estimated-vs-actual operator feedback is
        // recorded after the query completes.
        context.feedback_history = Some(self.optimizer_engine.feedback_history());
        // Columnar auto-detection (phase 2): share the engine's policy so the
        // typed columnar layout adapts to the learned hit rate; each query
        // merges its columnar stats back at completion.
        context.columnar_policy = Some(self.optimizer_engine.columnar_policy());
        context
    }
}
