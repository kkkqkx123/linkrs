use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::types::SpaceInfo;
use crate::core::types::TransactionId;
use crate::query::executor::base::ExecutionResult;
use crate::query::executor::streaming::instance::ResultSink;
use crate::query::executor::streaming::transaction_scope::TransactionScope;
use crate::query::executor::streaming::StreamingQueryResult;
use crate::query::parser::ast::Stmt;
use crate::query::validator::ValidatedStatement;
use crate::query::QueryContext;
use crate::query::QueryRequestContext;
use crate::storage::QueryStorage;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

/// Classification of a prepared statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementClass {
    ReadOnly,
    Dml,
    Ddl,
    Transaction,
    Diagnostic,
}

/// Check whether a statement performs any write operations to storage.
///
/// This detects both standalone DML (INSERT/DELETE/UPDATE) and MATCH
/// statements with embedded DELETE clauses.
pub fn requires_write_storage(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Match(m) => m.delete_clause.is_some(),
        _ => requires_auto_commit(stmt),
    }
}

/// A fully prepared request ready for execution.
///
/// Contains everything needed to compile, execute, and finalize a query:
/// the parsed+validated AST, query context, identity, and lifecycle metadata.
pub struct PreparedRequest {
    pub query_text: String,
    pub validated: ValidatedStatement,
    pub query_context: Arc<QueryContext>,
    pub statement_class: StatementClass,
    pub transaction_scope: TransactionScope,
    pub operation_storage: Option<Arc<RwLock<dyn QueryStorage>>>,
}

// ── Statement classification ───────────────────────────────────────────────

pub fn classify_statement(stmt: &Stmt) -> StatementClass {
    if is_diagnostic(stmt) {
        StatementClass::Diagnostic
    } else if is_transaction(stmt) {
        StatementClass::Transaction
    } else if is_ddl(stmt) {
        StatementClass::Ddl
    } else if requires_write_storage(stmt) {
        StatementClass::Dml
    } else {
        StatementClass::ReadOnly
    }
}

pub fn requires_auto_commit(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Insert(..)
            | Stmt::Update(..)
            | Stmt::Delete(..)
            | Stmt::Set(..)
            | Stmt::Remove(..)
            | Stmt::Merge(..)
    )
}

pub fn is_transaction(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::BeginTransaction(..) | Stmt::CommitTransaction(..) | Stmt::RollbackTransaction(..)
    )
}

pub fn is_ddl(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Create(_)
            | Stmt::Drop(_)
            | Stmt::Alter(_)
            | Stmt::ClearSpace(_)
            | Stmt::CreateFulltextIndex(_)
            | Stmt::DropFulltextIndex(_)
            | Stmt::AlterFulltextIndex(_)
            | Stmt::CreateVectorIndex(_)
            | Stmt::DropVectorIndex(_)
    )
}

pub fn is_diagnostic(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Explain(_) | Stmt::Profile(_))
}

pub fn is_read_only_cacheable(stmt: &Stmt) -> bool {
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

// ── Prepared lifecycle ─────────────────────────────────────────────────────

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    /// Parse and validate once, producing a [`PreparedRequest`].
    pub(crate) fn prepare_request(
        &mut self,
        query_text: &str,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<PreparedRequest> {
        let query_context = self.query_context_for_request(rctx, space_info.as_ref());
        let parser_result = self.parse_into_context(query_text)?;
        let validated = self.validate_parsed_statement(parser_result.ast, query_context.clone())?;
        Self::finalize_prepare(query_text, query_context, validated, None)
    }

    /// Parse and validate, binding auto-commit storage for DML before constructing the context.
    ///
    /// The storage is acquired before parsing so that the request context
    /// carries the real transaction identity from the storage binding.
    pub(crate) fn prepare_request_with_auto_commit(
        &mut self,
        query_text: &str,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<PreparedRequest> {
        let parser_result = self.parse_into_context(query_text)?;
        let needs_write = requires_write_storage(parser_result.ast.stmt());
        let operation_storage = if needs_write {
            Some(self.bind_auto_commit_storage()?)
        } else {
            None
        };

        let mut rctx = QueryRequestContext::new(query_text.to_string());
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
        Self::finalize_prepare(query_text, query_context, validated, operation_storage)
    }

    fn finalize_prepare(
        query_text: &str,
        query_context: Arc<QueryContext>,
        validated: ValidatedStatement,
        operation_storage: Option<Arc<RwLock<dyn QueryStorage>>>,
    ) -> DBResult<PreparedRequest> {
        let stmt = validated.ast.stmt();
        let statement_class = classify_statement(stmt);
        let transaction_scope =
            Self::resolve_transaction_scope(stmt, query_context.request_context());
        Ok(PreparedRequest {
            query_text: query_text.to_string(),
            validated,
            query_context,
            statement_class,
            transaction_scope,
            operation_storage,
        })
    }

    /// Compile (or get cached) and execute a prepared request with materialize sink.
    pub(crate) fn execute_prepared(
        &mut self,
        request: &PreparedRequest,
        transaction_id: Option<TransactionId>,
    ) -> DBResult<ExecutionResult> {
        if request.statement_class == StatementClass::Diagnostic {
            return self.execute_diagnostic(request);
        }
        let physical_plan = self.compile_or_get_cached(
            &request.query_text,
            request.query_context.clone(),
            &request.validated,
        )?;
        let start = Instant::now();
        let scope = transaction_id
            .map(|id| TransactionScope::explicit(id, true))
            .unwrap_or_else(|| request.transaction_scope.clone());
        let result = self.execute_compiled_with_scope(
            physical_plan,
            request.query_context.clone(),
            ResultSink::Materialize,
            scope,
        )?;
        self.record_cache_execution(
            &request.query_text,
            &request.query_context,
            request.validated.ast.stmt(),
            start.elapsed().as_secs_f64() * 1000.0,
        );
        if request.statement_class == StatementClass::Ddl {
            self.invalidate_after_ddl(request.query_context.space_name().as_deref());
        }
        Ok(result)
    }

    pub(crate) fn execute_prepared_materialized(
        &mut self,
        request: &PreparedRequest,
    ) -> DBResult<ExecutionResult> {
        if request.statement_class == StatementClass::Diagnostic {
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
        if request.statement_class == StatementClass::Ddl {
            self.invalidate_after_ddl(request.query_context.space_name().as_deref());
        }
        Ok(result)
    }

    pub(crate) fn execute_prepared_streaming(
        &mut self,
        request: &PreparedRequest,
        transaction_id: Option<TransactionId>,
    ) -> DBResult<StreamingQueryResult> {
        if request.statement_class == StatementClass::Diagnostic {
            let result = self.execute_diagnostic(request)?;
            return Ok(StreamingQueryResult::from_execution_result(result));
        }
        if request.statement_class == StatementClass::Ddl {
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
            Stmt::Explain(ref explain_stmt) => {
                self.execute_explain(explain_stmt, request.query_context.clone())
            }
            Stmt::Profile(ref profile_stmt) => {
                self.execute_profile(profile_stmt, request.query_context.clone())
            }
            _ => Err(DBError::from(QueryError::execution(
                "Not a diagnostic statement".to_string(),
            ))),
        }
    }

    // ── Request context construction ──────────────────────────────────────

    /// Build a [`QueryContext`] from a request context and optional space info.
    pub(crate) fn query_context_for_request(
        &self,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<&SpaceInfo>,
    ) -> Arc<QueryContext> {
        let mut query_context = QueryContext::new(rctx);
        if let Some(space) = space_info {
            query_context.set_space_info(space.clone());
        }
        Arc::new(query_context)
    }

    /// Validate a previously-parsed AST.
    pub(crate) fn validate_parsed_statement(
        &mut self,
        ast: Arc<crate::query::parser::ast::stmt::Ast>,
        query_context: Arc<QueryContext>,
    ) -> DBResult<ValidatedStatement> {
        let validation_info = self.validate_query_with_context(ast.clone(), query_context)?;
        Ok(ValidatedStatement::new(ast, validation_info))
    }

    // ── Transaction scope resolution ──────────────────────────────────────

    /// Resolve the [`TransactionScope`] from a statement and request context.
    pub(crate) fn resolve_transaction_scope(
        stmt: &Stmt,
        request: &QueryRequestContext,
    ) -> TransactionScope {
        if let Some(scope) = Self::scope_for_bound_request(request) {
            return scope;
        }
        if let Some(transaction_id) = request.transaction_id {
            if request.auto_commit {
                TransactionScope::auto_commit(transaction_id)
            } else {
                TransactionScope::explicit(transaction_id, !request.read_only)
            }
        } else if requires_auto_commit(stmt) {
            TransactionScope::None
        } else if is_transaction(stmt) {
            TransactionScope::CommandScope
        } else {
            TransactionScope::None
        }
    }

    fn scope_for_bound_request(request: &QueryRequestContext) -> Option<TransactionScope> {
        request
            .transaction_id
            .or_else(|| {
                request
                    .operation_context
                    .as_ref()
                    .and_then(|context| context.transaction_id)
            })
            .map(|transaction_id| {
                if request.auto_commit {
                    TransactionScope::auto_commit(transaction_id)
                } else {
                    TransactionScope::explicit(transaction_id, !request.read_only)
                }
            })
    }

    // ── Operation storage lifecycle ────────────────────────────────────────

    pub(crate) fn bind_auto_commit_storage(&self) -> DBResult<Arc<RwLock<dyn QueryStorage>>> {
        let storage = self.storage.as_ref().ok_or_else(|| {
            DBError::from(QueryError::execution(
                "DML requires a storage binding".to_string(),
            ))
        })?;
        let bound = storage
            .read()
            .bind_auto_commit_context()
            .map_err(|error| DBError::from(QueryError::execution(error.to_string())))?;
        Ok(Arc::new(RwLock::new(bound)))
    }

    pub(crate) fn finalize_operation_storage(
        &self,
        storage: Option<&Arc<RwLock<dyn QueryStorage>>>,
        committed: bool,
    ) -> DBResult<()> {
        if let Some(storage) = storage {
            storage
                .write()
                .finalize_operation(committed)
                .map_err(|error| DBError::from(QueryError::execution(error.to_string())))?;
        }
        Ok(())
    }

    // ── Cache helpers ──────────────────────────────────────────────────────

    pub(crate) fn invalidate_after_ddl(&self, space_name: Option<&str>) {
        self.schema_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.index_generation
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
        stmt: &Stmt,
        execution_time_ms: f64,
    ) {
        if !is_read_only_cacheable(stmt) {
            return;
        }
        let space_name = query_context
            .space_name()
            .or_else(|| query_context.request_context().space_name.clone());
        let schema_version = Some(
            self.schema_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        let index_version = Some(
            self.index_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        self.plan_cache.record_execution_with_space(
            query_text,
            execution_time_ms,
            space_name,
            schema_version,
            index_version,
        );
    }

    fn attach_stream_cache_execution_stats(
        &self,
        stream: &StreamingQueryResult,
        request: &PreparedRequest,
    ) {
        if !is_read_only_cacheable(request.validated.ast.stmt()) {
            return;
        }
        let space_name = request
            .query_context
            .space_name()
            .or_else(|| request.query_context.request_context().space_name.clone());
        let schema_version = Some(
            self.schema_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        );
        let index_version = Some(
            self.index_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        );

        let plan_cache = Arc::clone(&self.plan_cache);
        let query_text = request.query_text.clone();
        let space_name2 = space_name.clone();
        let execution_start = Instant::now();
        stream.set_on_drop(Box::new(move || {
            plan_cache.record_execution_with_space(
                &query_text,
                execution_start.elapsed().as_secs_f64() * 1000.0,
                space_name2,
                schema_version,
                index_version,
            );
        }));
    }
}
