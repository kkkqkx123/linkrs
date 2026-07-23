use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::types::SpaceInfo;
use crate::core::types::TransactionId;
use crate::query::executor::base::ExecutionResult;
use crate::query::executor::streaming::instance::{QueryBindings, ResultSink};
use crate::query::executor::streaming::plan::PhysicalPlan;
use crate::query::executor::streaming::transaction_scope::TransactionScope;
use crate::query::executor::streaming::StreamingQueryResult;
use crate::query::validator::ValidatedStatement;
use crate::query::QueryContext;
use crate::storage::QueryStorage;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

pub struct PreparedRequest {
    pub query_text: String,
    pub validated: ValidatedStatement,
    pub query_context: Arc<QueryContext>,
    pub transaction_scope: TransactionScope,
}

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub(crate) fn prepare_request(
        &mut self,
        query_text: &str,
        rctx: Arc<crate::query::QueryRequestContext>,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<PreparedRequest> {
        let (query_context, validated) =
            self.parse_and_validate_request(query_text, rctx, space_info.as_ref())?;
        let stmt = validated.ast.stmt();
        let transaction_scope = if Self::statement_requires_auto_commit(stmt) {
            Self::scope_for_request(stmt, query_context.request_context())
        } else if Self::statement_is_transaction(stmt) {
            TransactionScope::CommandScope
        } else {
            TransactionScope::None
        };
        Ok(PreparedRequest {
            query_text: query_text.to_string(),
            validated,
            query_context,
            transaction_scope,
        })
    }

    pub(crate) fn prepare_request_with_auto_commit(
        &mut self,
        query_text: &str,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<(PreparedRequest, Option<Arc<RwLock<dyn QueryStorage>>>)> {
        let parser_result = self.parse_into_context(query_text)?;
        let parsed_is_dml = Self::statement_requires_auto_commit(parser_result.ast.stmt());
        let operation_storage = if parsed_is_dml {
            Some(self.bind_auto_commit_storage()?)
        } else {
            None
        };

        let mut rctx = crate::query::QueryRequestContext::new(query_text.to_string());
        let space_name = space_info.as_ref().map(|s| s.space_name.clone());
        if let Some(ref name) = space_name {
            rctx.space_name = Some(name.clone());
        }
        if let Some(ref storage) = operation_storage {
            rctx.transaction_id = storage
                .read()
                .operation_context()
                .and_then(|context| context.transaction_id);
            rctx.operation_context = storage.read().operation_context().as_deref().cloned();
            rctx.operation_storage = Some(storage.clone());
        }

        let rctx = Arc::new(rctx);
        let query_context = self.query_context_for_request(rctx, space_info.as_ref());
        let validated = self.validate_parsed_statement(parser_result.ast, query_context.clone())?;
        let stmt = validated.ast.stmt();
        let transaction_scope = if Self::statement_requires_auto_commit(stmt) {
            Self::scope_for_request(stmt, query_context.request_context())
        } else if Self::statement_is_transaction(stmt) {
            TransactionScope::CommandScope
        } else {
            TransactionScope::None
        };
        Ok((
            PreparedRequest {
                query_text: query_text.to_string(),
                validated,
                query_context,
                transaction_scope,
            },
            operation_storage,
        ))
    }

    pub(crate) fn execute_prepared_materialized(
        &mut self,
        request: &PreparedRequest,
    ) -> DBResult<ExecutionResult> {
        if Self::is_explain_or_profile(request.validated.ast.stmt()) {
            return self.execute_diagnostic(request);
        }
        let physical_plan = self.compile_or_get_cached(
            &request.query_text,
            request.query_context.clone(),
            &request.validated,
        )?;
        let execution_start = Instant::now();
        let result = self.execute_compiled_with_scope(
            physical_plan,
            request.query_context.clone(),
            ResultSink::Materialize,
            request.transaction_scope.clone(),
        )?;
        self.record_cache_execution(
            &request.query_text,
            &request.query_context,
            request.validated.ast.stmt(),
            execution_start.elapsed().as_secs_f64() * 1000.0,
        );
        if Self::statement_is_ddl(request.validated.ast.stmt()) {
            self.invalidate_after_ddl(request.query_context.space_name().as_deref());
        }
        Ok(result)
    }

    pub(crate) fn execute_prepared_streaming(
        &mut self,
        request: &PreparedRequest,
        transaction_id: Option<TransactionId>,
    ) -> DBResult<StreamingQueryResult> {
        if Self::is_explain_or_profile(request.validated.ast.stmt()) {
            let result = self.execute_diagnostic(request)?;
            return Ok(StreamingQueryResult::from_execution_result(result));
        }
        if Self::statement_is_ddl(request.validated.ast.stmt()) {
            let result = self.execute_prepared_materialized(request)?;
            return Ok(StreamingQueryResult::from_execution_result(result));
        }
        let physical_plan = self.compile_or_get_cached(
            &request.query_text,
            request.query_context.clone(),
            &request.validated,
        )?;
        let scope = transaction_id
            .map(|id| TransactionScope::explicit(id, true))
            .unwrap_or_else(|| request.transaction_scope.clone());
        let stream = self.execute_compiled_stream_with_scope(
            physical_plan,
            request.query_context.clone(),
            scope,
        )?;
        self.attach_stream_cache_execution_stats(&stream, request);
        Ok(stream)
    }

    pub(crate) fn execute_diagnostic(
        &mut self,
        request: &PreparedRequest,
    ) -> DBResult<ExecutionResult> {
        match request.validated.ast.stmt() {
            crate::query::parser::ast::Stmt::Explain(ref explain_stmt) => {
                self.execute_explain(explain_stmt, request.query_context.clone())
            }
            crate::query::parser::ast::Stmt::Profile(ref profile_stmt) => {
                self.execute_profile(profile_stmt, request.query_context.clone())
            }
            _ => Err(DBError::from(QueryError::execution(
                "Not a diagnostic statement".to_string(),
            ))),
        }
    }

    pub(crate) fn is_explain_or_profile(stmt: &crate::query::parser::ast::Stmt) -> bool {
        matches!(
            stmt,
            crate::query::parser::ast::Stmt::Explain(_)
                | crate::query::parser::ast::Stmt::Profile(_)
        )
    }

    pub(crate) fn statement_requires_auto_commit(stmt: &crate::query::parser::ast::Stmt) -> bool {
        matches!(
            stmt,
            crate::query::parser::ast::Stmt::Insert(..)
                | crate::query::parser::ast::Stmt::Update(..)
                | crate::query::parser::ast::Stmt::Delete(..)
                | crate::query::parser::ast::Stmt::Set(..)
                | crate::query::parser::ast::Stmt::Remove(..)
                | crate::query::parser::ast::Stmt::Merge(..)
        )
    }

    pub(crate) fn statement_is_transaction(stmt: &crate::query::parser::ast::Stmt) -> bool {
        matches!(
            stmt,
            crate::query::parser::ast::Stmt::BeginTransaction(..)
                | crate::query::parser::ast::Stmt::CommitTransaction(..)
                | crate::query::parser::ast::Stmt::RollbackTransaction(..)
        )
    }

    pub(crate) fn statement_is_ddl(stmt: &crate::query::parser::ast::Stmt) -> bool {
        matches!(
            stmt,
            crate::query::parser::ast::Stmt::Create(_)
                | crate::query::parser::ast::Stmt::Drop(_)
                | crate::query::parser::ast::Stmt::Alter(_)
                | crate::query::parser::ast::Stmt::ClearSpace(_)
                | crate::query::parser::ast::Stmt::CreateFulltextIndex(_)
                | crate::query::parser::ast::Stmt::DropFulltextIndex(_)
                | crate::query::parser::ast::Stmt::AlterFulltextIndex(_)
                | crate::query::parser::ast::Stmt::CreateVectorIndex(_)
                | crate::query::parser::ast::Stmt::DropVectorIndex(_)
        )
    }

    pub(crate) fn invalidate_after_ddl(&self, space_name: Option<&str>) {
        self.schema_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(space_name) = space_name {
            let removed = self.plan_cache.invalidate_space(space_name);
            if removed > 0 {
                log::info!(
                    "Invalidated {} cached plans for space '{}' after committed DDL",
                    removed,
                    space_name
                );
            }
        } else {
            self.plan_cache.clear();
        }
    }

    pub(crate) fn record_cache_execution(
        &self,
        query_text: &str,
        query_context: &QueryContext,
        stmt: &crate::query::parser::ast::Stmt,
        execution_time_ms: f64,
    ) {
        if !Self::is_read_only_cacheable(stmt) {
            return;
        }
        let (optimizer_version, planning_config_hash) = self.optimizer_engine.cache_compatibility();
        let (cache_context, _) = self.plan_cache_context(
            query_text,
            query_context,
            optimizer_version,
            planning_config_hash,
        );
        if cache_context.space_name.is_some() {
            self.plan_cache.record_execution_with_context(
                query_text,
                execution_time_ms,
                &cache_context,
            );
        }
    }

    fn attach_stream_cache_execution_stats(
        &self,
        stream: &StreamingQueryResult,
        request: &PreparedRequest,
    ) {
        if !Self::is_read_only_cacheable(request.validated.ast.stmt()) {
            return;
        }
        let (optimizer_version, planning_config_hash) = self.optimizer_engine.cache_compatibility();
        let (cache_context, _) = self.plan_cache_context(
            &request.query_text,
            &request.query_context,
            optimizer_version,
            planning_config_hash,
        );
        if cache_context.space_name.is_none() {
            return;
        }

        let plan_cache = Arc::clone(&self.plan_cache);
        let query_text = request.query_text.clone();
        let execution_start = Instant::now();
        stream.set_on_drop(Box::new(move || {
            plan_cache.record_execution_with_context(
                &query_text,
                execution_start.elapsed().as_secs_f64() * 1000.0,
                &cache_context,
            );
        }));
    }

    pub(crate) fn is_read_only_cacheable(stmt: &crate::query::parser::ast::Stmt) -> bool {
        use crate::query::parser::ast::Stmt;
        !matches!(
            stmt,
            Stmt::Insert(_)
                | Stmt::Update(_)
                | Stmt::Delete(_)
                | Stmt::Set(_)
                | Stmt::Remove(_)
                | Stmt::Merge(_)
                | Stmt::Create(_)
                | Stmt::Drop(_)
                | Stmt::Alter(_)
                | Stmt::ClearSpace(_)
                | Stmt::CreateFulltextIndex(_)
                | Stmt::DropFulltextIndex(_)
                | Stmt::AlterFulltextIndex(_)
                | Stmt::CreateVectorIndex(_)
                | Stmt::DropVectorIndex(_)
                | Stmt::BeginTransaction(_)
                | Stmt::CommitTransaction(_)
                | Stmt::RollbackTransaction(_)
                | Stmt::Explain(_)
                | Stmt::Profile(_)
        )
    }

    pub(crate) fn scope_for_request(
        stmt: &crate::query::parser::ast::Stmt,
        request: &crate::query::QueryRequestContext,
    ) -> TransactionScope {
        if let Some(transaction_id) = request.transaction_id {
            if request.auto_commit {
                TransactionScope::auto_commit(transaction_id)
            } else {
                TransactionScope::explicit(transaction_id, !request.read_only)
            }
        } else if let Some(transaction_id) = request
            .operation_context
            .as_ref()
            .and_then(|context| context.transaction_id)
        {
            if request.auto_commit {
                TransactionScope::auto_commit(transaction_id)
            } else {
                TransactionScope::explicit(transaction_id, !request.read_only)
            }
        } else if Self::statement_requires_auto_commit(stmt) {
            TransactionScope::None
        } else if Self::statement_is_transaction(stmt) {
            TransactionScope::CommandScope
        } else {
            TransactionScope::None
        }
    }
}
