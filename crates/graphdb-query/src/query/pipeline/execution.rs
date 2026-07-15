use super::next_transaction_id;
use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::types::SpaceInfo;
use crate::core::{
    ErrorInfo, ErrorType, MetricType, QueryMetrics, QueryPhase, QueryProfile, StatsManager,
};
use crate::query::executor::base::{ExecutionContext, ExecutionResult};
use crate::query::executor::streaming::instance::{
    QueryBindings, QueryExecutionInstance, ResultSink,
};
use crate::query::executor::streaming::plan::PhysicalPlan;
use crate::query::executor::streaming::transaction_scope::TransactionScope;
use crate::query::executor::streaming::{StreamingQueryExecutor, StreamingQueryResult};
use crate::query::validator::ValidatedStatement;
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

    pub fn execute_query_with_space(
        &mut self,
        query_text: &str,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<ExecutionResult> {
        let mut rctx = QueryRequestContext::new(query_text.to_string());

        let space_name = space_info.as_ref().map(|s| s.space_name.clone());
        if let Some(ref name) = space_name {
            rctx.space_name = Some(name.clone());
        }

        let rctx = Arc::new(rctx);
        let mut query_context = QueryContext::new(rctx);

        if let Some(ref space) = space_info {
            query_context.set_space_info(space.clone());
        }

        let query_context = Arc::new(query_context);

        let schema_version = Some(
            self.schema_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        let index_version = schema_version;

        if let Some(cached_plan) = self.plan_cache.get_with_full_space(
            query_text,
            space_name.clone(),
            schema_version,
            index_version,
        ) {
            log::debug!("Query plan cache hit");
            let execute_start = Instant::now();
            let txn_scope = if cached_plan.is_dml {
                TransactionScope::auto_commit(crate::core::types::TransactionId::new(
                    next_transaction_id(),
                ))
            } else {
                TransactionScope::None
            };
            let result = self.execute_compiled_with_scope(
                cached_plan.plan.clone(),
                query_context.clone(),
                ResultSink::Materialize,
                txn_scope,
            )?;
            let execution_time_ms = execute_start.elapsed().as_millis() as f64;
            self.plan_cache.record_execution_with_space(
                query_text,
                execution_time_ms,
                space_name,
                schema_version,
                index_version,
            );
            return Ok(result);
        }

        let parser_result = self.parse_into_context(query_text)?;

        let validation_info =
            self.validate_query_with_context(parser_result.ast.clone(), query_context.clone())?;

        let validated = ValidatedStatement::new(parser_result.ast.clone(), validation_info);

        match validated.ast.stmt() {
            crate::query::parser::ast::Stmt::Explain(explain_stmt) => {
                return self.execute_explain(explain_stmt, query_context);
            }
            crate::query::parser::ast::Stmt::Profile(profile_stmt) => {
                return self.execute_profile(profile_stmt, query_context);
            }
            _ => {}
        }

        let is_dml = matches!(
            validated.ast.stmt(),
            crate::query::parser::ast::Stmt::Insert(_)
                | crate::query::parser::ast::Stmt::Update(_)
                | crate::query::parser::ast::Stmt::Delete(_)
                | crate::query::parser::ast::Stmt::Merge(_)
        );
        let is_ddl = matches!(
            validated.ast.stmt(),
            crate::query::parser::ast::Stmt::Create(_)
                | crate::query::parser::ast::Stmt::Drop(_)
                | crate::query::parser::ast::Stmt::Alter(_)
                | crate::query::parser::ast::Stmt::ClearSpace(_)
                | crate::query::parser::ast::Stmt::CreateFulltextIndex(_)
                | crate::query::parser::ast::Stmt::DropFulltextIndex(_)
                | crate::query::parser::ast::Stmt::AlterFulltextIndex(_)
                | crate::query::parser::ast::Stmt::CreateVectorIndex(_)
                | crate::query::parser::ast::Stmt::DropVectorIndex(_)
        );
        if is_ddl {
            self.schema_generation
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(ref name) = space_name {
                let removed = self.plan_cache.invalidate_space(name);
                if removed > 0 {
                    log::info!(
                        "Invalidated {} cached plans for space '{}' after DDL",
                        removed,
                        name
                    );
                }
            }
        }

        let execute_start = Instant::now();
        let (physical_plan, optimized_plan) = self.compile(query_context.clone(), &validated)?;

        let result = if optimized_plan.partition_spec().is_some() {
            self.execute_plan(query_context.clone(), optimized_plan)?
        } else {
            let txn_scope = if is_dml {
                TransactionScope::auto_commit(crate::core::types::TransactionId::new(
                    next_transaction_id(),
                ))
            } else {
                TransactionScope::None
            };
            self.execute_compiled_with_scope(
                physical_plan.clone(),
                query_context.clone(),
                ResultSink::Materialize,
                txn_scope,
            )?
        };
        let execution_time_ms = execute_start.elapsed().as_millis() as f64;

        let should_cache = !matches!(
            validated.ast.stmt(),
            crate::query::parser::ast::Stmt::Insert(_)
        );
        if should_cache {
            let param_positions = self.param_handler.extract_params(query_text);

            let dependent_tables: Vec<String> = validated
                .validation_info
                .semantic_info
                .referenced_tags
                .iter()
                .chain(
                    validated
                        .validation_info
                        .semantic_info
                        .referenced_edges
                        .iter(),
                )
                .cloned()
                .collect();

            let index_version = schema_version;

            self.plan_cache.put_with_context(
                query_text,
                physical_plan,
                param_positions,
                dependent_tables,
                space_name.clone(),
                schema_version,
                index_version,
                is_dml,
            );
            self.plan_cache.record_execution_with_space(
                query_text,
                execution_time_ms,
                space_name,
                schema_version,
                index_version,
            );
        }

        Ok(result)
    }

    pub fn execute_query_with_streaming(
        &mut self,
        query_text: &str,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<ExecutionResult> {
        self.execute_query_with_space(query_text, space_info)
    }

    pub fn execute_query_stream_with_request(
        &mut self,
        query_text: &str,
        rctx: Arc<crate::query::QueryRequestContext>,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<StreamingQueryResult> {
        let mut query_context = QueryContext::new(rctx);
        if let Some(ref space) = space_info {
            query_context.set_space_info(space.clone());
        }
        let query_context = Arc::new(query_context);

        let parser_result = self.parse_into_context(query_text)?;
        let validation_info =
            self.validate_query_with_context(parser_result.ast.clone(), query_context.clone())?;
        let validated = ValidatedStatement::new(parser_result.ast.clone(), validation_info);

        match validated.ast.stmt() {
            crate::query::parser::ast::Stmt::Explain(explain_stmt) => {
                let result = self.execute_explain(explain_stmt, query_context)?;
                return Ok(StreamingQueryResult::from_execution_result(result));
            }
            crate::query::parser::ast::Stmt::Profile(profile_stmt) => {
                let result = self.execute_profile(profile_stmt, query_context)?;
                return Ok(StreamingQueryResult::from_execution_result(result));
            }
            _ => {}
        }

        let (physical_plan, optimized_plan) = self.compile(query_context.clone(), &validated)?;

        if optimized_plan.partition_spec().is_some() {
            self.execute_plan_to_stream(query_context, optimized_plan)
        } else {
            self.execute_compiled_stream(physical_plan, query_context)
        }
    }

    pub fn execute_query_with_request(
        &mut self,
        query_text: &str,
        rctx: Arc<crate::query::QueryRequestContext>,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<ExecutionResult> {
        let mut query_context = QueryContext::new(rctx);
        if let Some(ref space) = space_info {
            query_context.set_space_info(space.clone());
        }
        let query_context = Arc::new(query_context);

        let parser_result = self.parse_into_context(query_text)?;
        let validation_info =
            self.validate_query_with_context(parser_result.ast.clone(), query_context.clone())?;
        let validated = ValidatedStatement::new(parser_result.ast.clone(), validation_info);

        match validated.ast.stmt() {
            crate::query::parser::ast::Stmt::Explain(explain_stmt) => {
                return self.execute_explain(explain_stmt, query_context);
            }
            crate::query::parser::ast::Stmt::Profile(profile_stmt) => {
                return self.execute_profile(profile_stmt, query_context);
            }
            _ => {}
        }

        let (physical_plan, optimized_plan) = self.compile(query_context.clone(), &validated)?;

        if optimized_plan.partition_spec().is_some() {
            self.execute_plan(query_context, optimized_plan)
        } else {
            self.execute_compiled(physical_plan, query_context, ResultSink::Materialize)
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

        let rctx = Arc::new(QueryRequestContext::new(query_text.to_string()));
        let query_context = Arc::new(QueryContext::new(rctx));

        let parse_start = Instant::now();
        let parser_result = match self.parse_into_context(query_text) {
            Ok(result) => {
                profile.stages.parse_us = parse_start.elapsed().as_micros() as u64;
                metrics.record_parse_time(parse_start.elapsed());
                result
            }
            Err(e) => {
                profile.stages.parse_us = parse_start.elapsed().as_micros() as u64;
                let error_info =
                    ErrorInfo::new(ErrorType::ParseError, QueryPhase::Parse, e.to_string());
                profile.mark_failed_with_info(error_info.clone());
                profile.total_duration_us = total_start.elapsed().as_micros() as u64;
                self.stats_manager
                    .record_failed_query(profile.clone(), error_info);
                return Err(e);
            }
        };

        self.record_query_type_counter(parser_result.ast.stmt());

        let validate_start = Instant::now();
        let validation_info = match self
            .validate_query_with_context(parser_result.ast.clone(), query_context.clone())
        {
            Ok(info) => info,
            Err(e) => {
                profile.stages.validate_us = validate_start.elapsed().as_micros() as u64;
                let error_info = ErrorInfo::new(
                    ErrorType::ValidationError,
                    QueryPhase::Validate,
                    e.to_string(),
                );
                profile.mark_failed_with_info(error_info.clone());
                profile.total_duration_us = total_start.elapsed().as_micros() as u64;
                self.stats_manager
                    .record_failed_query(profile.clone(), error_info);
                return Err(e);
            }
        };

        profile.stages.validate_us = validate_start.elapsed().as_micros() as u64;
        metrics.record_validate_time(validate_start.elapsed());

        let validated = ValidatedStatement::new(parser_result.ast.clone(), validation_info);

        match validated.ast.stmt() {
            crate::query::parser::ast::Stmt::Explain(explain_stmt) => {
                let result = self.execute_explain(explain_stmt, query_context)?;
                profile.total_duration_us = total_start.elapsed().as_micros() as u64;
                metrics.record_total_time(total_start.elapsed());
                return Ok((result, metrics, profile));
            }
            crate::query::parser::ast::Stmt::Profile(profile_stmt) => {
                let result = self.execute_profile(profile_stmt, query_context)?;
                profile.total_duration_us = total_start.elapsed().as_micros() as u64;
                metrics.record_total_time(total_start.elapsed());
                return Ok((result, metrics, profile));
            }
            _ => {}
        }

        let plan_start = Instant::now();
        let execution_plan = match self.generate_execution_plan(query_context.clone(), &validated) {
            Ok(plan) => {
                profile.stages.plan_us = plan_start.elapsed().as_micros() as u64;
                metrics.set_plan_node_count(plan.node_count());
                metrics.record_plan_time(plan_start.elapsed());
                plan
            }
            Err(e) => {
                profile.stages.plan_us = plan_start.elapsed().as_micros() as u64;
                let error_info =
                    ErrorInfo::new(ErrorType::PlanningError, QueryPhase::Plan, e.to_string());
                profile.mark_failed_with_info(error_info.clone());
                profile.total_duration_us = total_start.elapsed().as_micros() as u64;
                self.stats_manager
                    .record_failed_query(profile.clone(), error_info);
                return Err(e);
            }
        };

        let optimize_start = Instant::now();
        let optimized_plan = match self.optimize_execution_plan(execution_plan) {
            Ok(plan) => {
                profile.stages.optimize_us = optimize_start.elapsed().as_micros() as u64;
                metrics.record_optimize_time(optimize_start.elapsed());
                plan
            }
            Err(e) => {
                profile.stages.optimize_us = optimize_start.elapsed().as_micros() as u64;
                let error_info = ErrorInfo::new(
                    ErrorType::OptimizationError,
                    QueryPhase::Optimize,
                    e.to_string(),
                );
                profile.mark_failed_with_info(error_info.clone());
                profile.total_duration_us = total_start.elapsed().as_micros() as u64;
                self.stats_manager
                    .record_failed_query(profile.clone(), error_info);
                return Err(e);
            }
        };

        let physical_plan = self.build_physical_plan(&optimized_plan, &query_context)?;

        let execute_start = Instant::now();
        let has_partition = optimized_plan.partition_spec().is_some();
        let result = if has_partition {
            match self.execute_plan(query_context, optimized_plan) {
                Ok(result) => result,
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
            }
        } else {
            match self.execute_compiled(physical_plan, query_context, ResultSink::Materialize) {
                Ok(result) => result,
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

    pub(crate) fn execute_compiled(
        &self,
        physical_plan: Arc<PhysicalPlan>,
        query_context: Arc<QueryContext>,
        sink: ResultSink,
    ) -> DBResult<ExecutionResult> {
        self.execute_compiled_with_scope(physical_plan, query_context, sink, TransactionScope::None)
    }

    pub(crate) fn execute_compiled_with_scope(
        &self,
        physical_plan: Arc<PhysicalPlan>,
        query_context: Arc<QueryContext>,
        sink: ResultSink,
        transaction_scope: TransactionScope,
    ) -> DBResult<ExecutionResult> {
        let exec_ctx = self.build_execution_context(&query_context);
        let mut bindings = QueryBindings::from_context(&exec_ctx, transaction_scope);
        bindings.query_id = exec_ctx.query_id;

        let mut instance = QueryExecutionInstance::instantiate_plan(
            physical_plan,
            bindings,
            sink,
            self.query_registry.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

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
            ResultSink::Stream => {
                let _ = instance
                    .into_stream()
                    .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;
                Err(DBError::from(QueryError::execution(
                    "Use execute_compiled_stream for streaming sink".to_string(),
                )))
            }
        }
    }

    pub(crate) fn execute_compiled_stream(
        &self,
        physical_plan: Arc<PhysicalPlan>,
        query_context: Arc<QueryContext>,
    ) -> DBResult<StreamingQueryResult> {
        self.execute_compiled_stream_with_scope(
            physical_plan,
            query_context,
            TransactionScope::None,
        )
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

        let mut context = ExecutionContext::default();
        context.max_workers = self
            .optimizer_engine
            .partitioning_config()
            .max_workers
            .max(1);
        context.max_buffered_chunks = self
            .optimizer_engine
            .partitioning_config()
            .max_buffered_chunks
            .max(1);
        if let Some(ref storage) = self.storage {
            let dyn_storage: Arc<RwLock<dyn QueryStorage>> = storage.clone();
            context.storage = Some(dyn_storage);
        }
        #[cfg(feature = "fulltext-search")]
        {
            context.fulltext_manager = self.fulltext_manager.clone();
        }
        #[cfg(feature = "qdrant")]
        {
            context.vector_coordinator = self.vector_coordinator.clone();
        }
        if let Some(ref space_name) = query_context.space_name() {
            context.space_name = Some(space_name.clone());
        }
        let params: HashMap<String, crate::core::Value> =
            query_context.request_context().parameters.clone();
        context.parameters = Arc::new(params);
        context
    }

    pub(crate) fn execute_physical_plan(
        &self,
        physical_plan: Arc<PhysicalPlan>,
        query_context: Arc<QueryContext>,
    ) -> DBResult<ExecutionResult> {
        let exec_ctx = self.build_execution_context(&query_context);
        let mut bindings = QueryBindings::from_context(&exec_ctx, TransactionScope::None);
        bindings.query_id = exec_ctx.query_id;

        let mut instance = QueryExecutionInstance::instantiate_plan(
            physical_plan,
            bindings,
            ResultSink::Materialize,
            self.query_registry.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        instance
            .execute()
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))
    }

    #[allow(clippy::field_reassign_with_default)]
    pub(crate) fn execute_plan(
        &mut self,
        query_context: Arc<QueryContext>,
        plan: crate::query::planning::plan::ExecutionPlan,
    ) -> DBResult<ExecutionResult> {
        let has_partition = plan.partition_spec().is_some();

        if !has_partition {
            let physical_plan = self.build_physical_plan(&plan, &query_context)?;
            return self.execute_physical_plan(physical_plan, query_context);
        }

        log::debug!("Executing with StreamingExecutor (partitioned fallback)");

        let root_node = plan.root.as_ref().ok_or_else(|| {
            DBError::from(QueryError::execution("Empty execution plan".to_string()))
        })?;

        let mut executor = StreamingQueryExecutor::new();
        let context = self.build_execution_context(&query_context);
        let partition_spec = plan.partition_spec().unwrap();
        let physical_plan = crate::query::planning::plan::PartitionedPhysicalPlan::from_logical(
            root_node.clone(),
            partition_spec.clone(),
        );
        executor
            .from_partitioned_physical_plan(&physical_plan, &context)
            .map_err(|e| {
                DBError::from(QueryError::execution(format!(
                    "Failed to create streaming executor: {}",
                    e
                )))
            })?;

        executor.execute().map_err(|e| {
            DBError::from(QueryError::execution(format!(
                "Streaming execution failed: {:?}",
                e
            )))
        })
    }

    #[allow(clippy::field_reassign_with_default)]
    pub fn execute_plan_to_stream(
        &mut self,
        query_context: Arc<QueryContext>,
        plan: crate::query::planning::plan::ExecutionPlan,
    ) -> DBResult<StreamingQueryResult> {
        let has_partition = plan.partition_spec().is_some();

        if !has_partition {
            let physical_plan = self.build_physical_plan(&plan, &query_context)?;
            let exec_ctx = self.build_execution_context(&query_context);
            let mut bindings = QueryBindings::from_context(&exec_ctx, TransactionScope::None);
            bindings.query_id = exec_ctx.query_id;

            let instance = QueryExecutionInstance::instantiate_plan(
                physical_plan,
                bindings,
                ResultSink::Stream,
                self.query_registry.clone(),
            )
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

            return instance
                .into_stream()
                .map_err(|e| DBError::from(QueryError::execution(e.to_string())));
        }

        log::debug!("Executing with StreamingExecutor (stream, partitioned fallback)");

        let root_node = plan.root.as_ref().ok_or_else(|| {
            DBError::from(QueryError::execution("Empty execution plan".to_string()))
        })?;

        let mut executor = StreamingQueryExecutor::new();
        let context = self.build_execution_context(&query_context);
        let partition_spec = plan.partition_spec().unwrap();
        let physical_plan = crate::query::planning::plan::PartitionedPhysicalPlan::from_logical(
            root_node.clone(),
            partition_spec.clone(),
        );
        executor
            .from_partitioned_physical_plan(&physical_plan, &context)
            .map_err(|e| {
                DBError::from(QueryError::execution(format!(
                    "Failed to create streaming executor: {}",
                    e
                )))
            })?;

        let runtime = executor.runtime().cloned().ok_or_else(|| {
            DBError::from(QueryError::execution(
                "No execution runtime available".to_string(),
            ))
        })?;

        let stream = executor.into_stream().map_err(|e| {
            DBError::from(QueryError::execution(format!(
                "Failed to create result stream: {}",
                e
            )))
        })?;

        let result = StreamingQueryResult::new(stream, runtime);

        let col_names = root_node.col_names().to_vec();
        if !col_names.is_empty() {
            result.set_fallback_column_names(col_names);
        }

        Ok(result)
    }
}
