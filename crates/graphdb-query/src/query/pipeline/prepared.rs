use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::types::SpaceInfo;
use crate::core::types::TransactionId;
use crate::query::binder::BoundStatement;
use crate::query::executor::base::ExecutionResult;
use crate::query::executor::streaming::instance::ResultSink;
use crate::query::executor::streaming::transaction_scope::TransactionScope;
use crate::query::executor::streaming::StreamingQueryResult;
use crate::query::parser::ast::Stmt;
use crate::query::QueryContext;
use crate::query::QueryRequestContext;
use crate::storage::QueryStorage;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

/// Classification of a prepared statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementClass {
    Analyze,
    ReadOnly,
    Dml,
    Ddl,
    Transaction,
    Diagnostic,
}

/// Check whether a statement performs any write operations to storage.
///
/// This detects both standalone DML (INSERT/DELETE/UPDATE), MATCH statements
/// with embedded DELETE clauses, and DML nested inside pipe or set-operation
/// statements.
pub fn requires_write_storage(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Match(m) => m.delete_clause.is_some(),
        Stmt::Pipe(pipe) => {
            requires_write_storage(&pipe.left) || requires_write_storage(&pipe.right)
        }
        Stmt::SetOperation(set_op) => {
            requires_write_storage(&set_op.left) || requires_write_storage(&set_op.right)
        }
        _ => requires_auto_commit(stmt),
    }
}

/// Outcome of executing a prepared request.
///
/// Returned by [`QueryPipelineManager::execute_prepared`] regardless of the
/// sink: a materialized [`ExecutionResult`] or a streaming
/// [`StreamingQueryResult`].
pub(crate) enum PreparedOutcome {
    /// Fully materialized result for `ResultSink::Materialize`.
    Materialized(ExecutionResult),
    /// Streaming result for `ResultSink::Stream`.
    Stream(StreamingQueryResult),
}

/// A fully prepared request ready for execution.
///
/// Contains everything needed to compile, execute, and finalize a query:
/// the bound statement IR, query context, identity, and lifecycle metadata.
pub struct PreparedRequest {
    pub query_text: String,
    pub query_context: Arc<QueryContext>,
    pub statement_class: StatementClass,
    pub transaction_scope: TransactionScope,
    pub operation_storage: Option<Arc<RwLock<dyn QueryStorage>>>,
    /// Whether `operation_storage` was auto-bound during `prepare_request`
    /// (i.e. not provided by the caller). The pipeline owns its lifecycle and
    /// must call `finalize_operation` after execution; otherwise one MVCC
    /// snapshot per statement is leaked, degrading loads to O(n²).
    pub owns_operation_storage: bool,
    /// Fully resolved bound IR, produced by the Binder.
    pub bound_statement: Option<BoundStatement>,
    /// Cloned AST statement for classification and diagnostic matching.
    pub stmt: Stmt,
    /// The parsed AST, retained for the legacy `transform()` planning path.
    pub ast: Arc<crate::query::parser::ast::stmt::Ast>,
    /// P1: whether the query text is a shape-normalized DML template that may
    /// reuse a cached physical plan with per-statement parameter values.
    pub dml_shape_cacheable: bool,
}

impl PreparedRequest {
    /// Finalize the auto-bound operation storage after execution.
    ///
    /// Unregisters the MVCC snapshots registered by `with_auto_commit_context()`
    /// during binding and commits/releases the write-timestamp lease. Skipping
    /// this leaks one `active_snapshots` entry per statement; every later
    /// `register_snapshot` then rescans the growing map to recompute
    /// `min_active_snapshot_ts`, degrading bulk loads to O(n²).
    ///
    /// No-op unless this request owns its operation storage (i.e. it was
    /// auto-bound by `prepare_request` rather than supplied by the caller).
    pub(crate) fn finalize_owned_operation(&self, committed: bool) -> DBResult<()> {
        if !self.owns_operation_storage {
            return Ok(());
        }
        if let Some(storage) = &self.operation_storage {
            storage
                .write()
                .finalize_operation(committed)
                .map_err(|error| DBError::from(QueryError::execution(error.to_string())))?;
        }
        Ok(())
    }
}

// ── Statement classification ───────────────────────────────────────────────

pub fn classify_statement(stmt: &Stmt) -> StatementClass {
    if is_diagnostic(stmt) {
        StatementClass::Diagnostic
    } else if is_analyze(stmt) {
        StatementClass::Analyze
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

pub fn is_analyze(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Analyze(_))
}

pub fn requires_auto_commit(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Pipe(pipe) => requires_auto_commit(&pipe.left) || requires_auto_commit(&pipe.right),
        Stmt::SetOperation(set_op) => {
            requires_auto_commit(&set_op.left) || requires_auto_commit(&set_op.right)
        }
        _ => is_direct_dml(stmt),
    }
}

/// Whether the statement is one of the direct DML forms eligible for shape
/// normalization. Shared with the shape-cache candidate check in
/// [`prepare_request`] so the set of write statements stays in one place.
fn is_direct_dml(stmt: &Stmt) -> bool {
    is_direct_dml_statement(stmt) || is_direct_dcl(stmt)
}

/// Whether the statement is one of the direct DML forms eligible for shape
/// normalization. Shared with the shape-cache candidate check in
/// [`prepare_request`] so the set of write statements stays in one place.
fn is_direct_dml_statement(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Insert(_)
            | Stmt::Delete(_)
            | Stmt::Update(_)
            | Stmt::Merge(_)
            | Stmt::Set(_)
            | Stmt::Remove(_)
    )
}

/// Direct DCL statements that write the user/privilege store: they must run
/// on the auto-commit write path, not the read-only snapshot path.
fn is_direct_dcl(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::CreateUser(_)
            | Stmt::AlterUser(_)
            | Stmt::DropUser(_)
            | Stmt::ChangePassword(_)
            | Stmt::Grant(_)
            | Stmt::Revoke(_)
            | Stmt::UpdateConfigs(_)
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
    match stmt {
        Stmt::Pipe(pipe) => {
            is_read_only_cacheable(&pipe.left) && is_read_only_cacheable(&pipe.right)
        }
        Stmt::SetOperation(set_op) => {
            is_read_only_cacheable(&set_op.left) && is_read_only_cacheable(&set_op.right)
        }
        _ => !matches!(
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
                | Stmt::Analyze(_)
        ),
    }
}

/// Build a minimal `ValidatedStatement` for the `plan_bound` → `transform` fallback path.
///
/// When a planner does not yet implement `plan_bound`, we fall back to the legacy
/// `transform` interface.  This helper constructs a lightweight `ValidatedStatement`
/// with an empty `ValidationInfo` — sufficient because `transform` primarily reads
/// from the AST, not from `ValidationInfo`.
pub(crate) fn build_validated_fallback(
    ast: &Arc<crate::query::parser::ast::stmt::Ast>,
) -> crate::query::binder::validation::ValidatedStatement {
    crate::query::binder::validation::ValidatedStatement::new(
        ast.clone(),
        crate::query::binder::validation::ValidationInfo::new(),
    )
}

// ── Prepared lifecycle ─────────────────────────────────────────────────────

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    /// Parse and bind once, producing a [`PreparedRequest`].
    pub(crate) fn prepare_request(
        &mut self,
        query_text: &str,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<PreparedRequest> {
        let mut parser_result = self.parse_into_context(query_text)?;

        // P1: shape-normalize DML statements so structurally identical
        // INSERT/UPDATE/DELETE reuse a cached physical plan, binding their
        // literal values as parameters at execution time.
        let (effective_query, effective_rctx, dml_shape_cacheable) = if self.dml_shape_cache_enabled
            && is_direct_dml(parser_result.ast.stmt())
            && !rctx.parameters.iter().any(|(name, _)| {
                name.starts_with(crate::query::planning::dml_shape::DML_PARAM_PREFIX)
            }) {
            if let Some(shape) =
                crate::query::planning::dml_shape::normalize_shape(parser_result.ast.stmt())
            {
                // The canonical template must re-parse faithfully. A failure here
                // means the renderer emitted text that is not round-trippable; fall
                // back to the non-cached path instead of failing the statement.
                match self.parse_into_context(&shape.normalized_text) {
                    Ok(normalized) => {
                        let mut updated = (*rctx).clone();
                        updated.query = shape.normalized_text.clone();
                        for (index, value) in shape.values.iter().enumerate() {
                            updated.parameters.insert(
                                format!(
                                    "{}{}",
                                    crate::query::planning::dml_shape::DML_PARAM_PREFIX,
                                    index
                                ),
                                value.clone(),
                            );
                        }
                        parser_result = normalized;
                        (shape.normalized_text, Arc::new(updated), true)
                    }
                    Err(_) => {
                        log::warn!(
                            "DML shape template failed to re-parse, falling back to non-cached path: {}",
                            shape.normalized_text
                        );
                        (query_text.to_string(), rctx, false)
                    }
                }
            } else {
                (query_text.to_string(), rctx, false)
            }
        } else {
            (query_text.to_string(), rctx, false)
        };

        let needs_write = requires_write_storage(parser_result.ast.stmt());
        // Read-only auto-commit statements get a statement-level snapshot
        // context too (T2): every operator observes one fixed read timestamp
        // and the per-table MVCC snapshots pin the versions until finalize.
        // DDL / diagnostic / ANALYZE / transaction statements are excluded:
        // DDL writes schema and must bind through the write path.
        let is_read_only_statement = !needs_write
            && !is_ddl(parser_result.ast.stmt())
            && !is_diagnostic(parser_result.ast.stmt())
            && !is_analyze(parser_result.ast.stmt())
            && !is_transaction(parser_result.ast.stmt());
        let auto_commit_needs_binding = effective_rctx.operation_storage.is_none()
            && effective_rctx.auto_commit
            && (needs_write || is_read_only_statement);

        let (operation_storage, effective_rctx, owns_operation_storage) =
            if auto_commit_needs_binding {
                let storage = if needs_write {
                    self.bind_auto_commit_storage()?
                } else {
                    self.bind_read_operation_storage()?
                };
                let mut updated = (*effective_rctx).clone();
                let op_ctx = storage.read().operation_context();
                updated.transaction_id = op_ctx.as_ref().and_then(|c| c.transaction_id);
                updated.operation_context = op_ctx.as_deref().cloned();
                updated.operation_storage = Some(storage.clone());
                (Some(storage), Arc::new(updated), true)
            } else {
                (
                    effective_rctx.operation_storage.clone(),
                    effective_rctx,
                    false,
                )
            };

        let query_context = self.query_context_for_request(effective_rctx, space_info.as_ref());
        let ast = parser_result.ast.clone();
        let bound = self.bind_parsed_statement(parser_result.ast, query_context.clone())?;
        Self::finalize_prepare(
            &effective_query,
            query_context,
            ast,
            operation_storage,
            owns_operation_storage,
            bound,
            dml_shape_cacheable,
        )
    }

    /// Parse and bind, with auto-commit storage for DML.
    ///
    /// Delegates to [`prepare_request`] which now handles auto-commit storage
    /// binding internally when `rctx.auto_commit` is true and the statement
    /// requires write storage.
    pub(crate) fn prepare_request_with_auto_commit(
        &mut self,
        query_text: &str,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<PreparedRequest> {
        let mut rctx = QueryRequestContext::new(query_text.to_string());
        if let Some(ref name) = space_info.as_ref().map(|s| s.space_name.clone()) {
            rctx.space_name = Some(name.clone());
        }
        // QueryRequestContext::new() already sets auto_commit: true
        self.prepare_request(query_text, Arc::new(rctx), space_info)
    }

    fn finalize_prepare(
        query_text: &str,
        query_context: Arc<QueryContext>,
        ast: Arc<crate::query::parser::ast::stmt::Ast>,
        operation_storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        owns_operation_storage: bool,
        bound_statement: Option<BoundStatement>,
        dml_shape_cacheable: bool,
    ) -> DBResult<PreparedRequest> {
        let stmt = ast.stmt().clone();
        let statement_class = classify_statement(&stmt);
        let transaction_scope =
            Self::resolve_transaction_scope(&stmt, query_context.request_context());
        Ok(PreparedRequest {
            query_text: query_text.to_string(),
            query_context,
            statement_class,
            transaction_scope,
            operation_storage,
            owns_operation_storage,
            bound_statement,
            stmt,
            ast,
            dml_shape_cacheable,
        })
    }

    /// Compile (or get cached) and execute a prepared request with a
    /// materialized or streaming sink, finalizing auto-bound operation
    /// storage on success/failure.
    ///
    /// Single unified entry point (replaces the three former
    /// `execute_prepared*` variants): diagnostic, analyze, and DDL
    /// statements take their dedicated paths, everything else shares one
    /// compile + execute core distinguished only by the sink.
    pub(crate) fn execute_prepared(
        &mut self,
        request: &PreparedRequest,
        transaction_id: Option<TransactionId>,
        sink: ResultSink,
    ) -> DBResult<PreparedOutcome> {
        match sink {
            ResultSink::Discard => Err(DBError::from(QueryError::execution(
                "Discard sink must be handled by the caller".to_string(),
            ))),
            ResultSink::Materialize => match self.execute_prepared_inner(request, None, sink) {
                Ok(outcome) => {
                    request.finalize_owned_operation(true)?;
                    Ok(outcome)
                }
                Err(error) => {
                    let _ = request.finalize_owned_operation(false);
                    Err(error)
                }
            },
            ResultSink::Stream => {
                match self.execute_prepared_inner(request, transaction_id, sink) {
                    Ok(PreparedOutcome::Stream(stream)) => {
                        if request.owns_operation_storage {
                            if let Some(storage) = request.operation_storage.clone() {
                                // Finalize when the stream ends: commit after full
                                // consumption, abort on error, cancellation, or drop.
                                let commit_storage = storage.clone();
                                let abort_storage = storage;
                                stream.set_transaction_finalizer_with_result(
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
                        Ok(PreparedOutcome::Stream(stream))
                    }
                    Ok(other) => Ok(other),
                    Err(error) => {
                        let _ = request.finalize_owned_operation(false);
                        Err(error)
                    }
                }
            }
        }
    }

    /// Shared inner execution core.
    ///
    /// Compile (or fetch from the plan cache), execute with the requested
    /// sink, and record cache/DDL bookkeeping.  The outer
    /// [`execute_prepared`](Self::execute_prepared) wrapper handles storage
    /// finalization; DDL executed through a streaming sink is materialized
    /// then re-wrapped as a stream.
    fn execute_prepared_inner(
        &mut self,
        request: &PreparedRequest,
        transaction_id: Option<TransactionId>,
        sink: ResultSink,
    ) -> DBResult<PreparedOutcome> {
        // Classification-specific paths: no plan is compiled.
        if request.statement_class == StatementClass::Diagnostic {
            return Ok(match sink {
                ResultSink::Materialize => {
                    PreparedOutcome::Materialized(self.execute_diagnostic(request)?)
                }
                ResultSink::Stream => PreparedOutcome::Stream(
                    StreamingQueryResult::from_execution_result(self.execute_diagnostic(request)?),
                ),
                ResultSink::Discard => unreachable!("discard sink is rejected by the caller"),
            });
        }
        if request.statement_class == StatementClass::Analyze {
            let result = self.execute_analyze(request)?;
            return Ok(match sink {
                ResultSink::Materialize => PreparedOutcome::Materialized(result),
                ResultSink::Stream => {
                    PreparedOutcome::Stream(StreamingQueryResult::from_execution_result(result))
                }
                ResultSink::Discard => unreachable!("discard sink is rejected by the caller"),
            });
        }
        // DDL has no streaming semantics: materialize and wrap.
        let stream_ddl = sink == ResultSink::Stream && request.statement_class == StatementClass::Ddl;

        let physical_plan = self.compile_or_get_cached(
            &request.query_text,
            request.query_context.clone(),
            request.bound_statement.as_ref().ok_or_else(|| {
                DBError::from(QueryError::execution("No bound statement".to_string()))
            })?,
            &request.stmt,
            &request.ast,
            request.dml_shape_cacheable,
        )?;
        let scope = transaction_id
            .map(|id| TransactionScope::explicit(id, true))
            .unwrap_or_else(|| request.transaction_scope.clone());

        if stream_ddl || sink == ResultSink::Materialize {
            let start = Instant::now();
            let result = self.execute_compiled_with_scope(
                physical_plan,
                request.query_context.clone(),
                ResultSink::Materialize,
                scope,
            )?;
            self.record_cache_execution(
                &request.query_text,
                &request.query_context,
                &request.stmt,
                start.elapsed().as_secs_f64() * 1000.0,
            );
            if request.statement_class == StatementClass::Ddl {
                self.invalidate_after_ddl(request.query_context.space_name().as_deref());
            }
            if stream_ddl {
                return Ok(PreparedOutcome::Stream(
                    StreamingQueryResult::from_execution_result(result),
                ));
            }
            return Ok(PreparedOutcome::Materialized(result));
        }

        let stream = self.execute_compiled_stream_with_scope(
            physical_plan,
            request.query_context.clone(),
            scope,
        )?;
        self.attach_stream_cache_execution_stats(&stream, request);
        Ok(PreparedOutcome::Stream(stream))
    }

    pub(crate) fn execute_diagnostic(
        &mut self,
        request: &PreparedRequest,
    ) -> DBResult<ExecutionResult> {
        match &request.stmt {
            Stmt::Explain(ref explain_stmt) => {
                if explain_stmt.analyze {
                    self.execute_explain_analyze(
                        explain_stmt,
                        request.query_context.clone(),
                        request.transaction_scope.clone(),
                    )
                } else {
                    self.execute_explain(explain_stmt, request.query_context.clone())
                }
            }
            Stmt::Profile(ref profile_stmt) => self.execute_profile(
                profile_stmt,
                request.query_context.clone(),
                request.transaction_scope.clone(),
            ),
            _ => Err(DBError::from(QueryError::execution(
                "Not a diagnostic statement".to_string(),
            ))),
        }
    }

    /// Execute an ANALYZE statement: collect statistics for the target space.
    ///
    /// This is a bypass path: no plan is generated, statistics are written to
    /// the optimizer's `StatisticsManager` only.
    pub(crate) fn execute_analyze(
        &mut self,
        request: &PreparedRequest,
    ) -> DBResult<ExecutionResult> {
        let space_name = match &request.stmt {
            Stmt::Analyze(analyze) => analyze
                .space
                .clone()
                .or_else(|| request.query_context.space_name())
                .or_else(|| request.query_context.request_context().space_name.clone()),
            _ => request.query_context.space_name(),
        };
        let space_name = space_name.ok_or_else(|| {
            DBError::from(QueryError::execution(
                "ANALYZE requires a space: use ANALYZE SPACE <name> or USE <space> first"
                    .to_string(),
            ))
        })?;
        self.collect_statistics(&space_name, true)
            .map_err(|error| DBError::from(QueryError::execution(error)))?;
        log::info!("ANALYZE completed for space '{}'", space_name);
        Ok(ExecutionResult::Success)
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

    /// Bind a read-only statement context with a fixed snapshot timestamp.
    ///
    /// Every storage access of the statement observes the same read
    /// timestamp, and per-table MVCC snapshots are lazily registered so GC
    /// cannot terminate versions the statement may still read. The bound
    /// handle is finalized (snapshot unregistration) by the same
    /// `finalize_owned_operation` lifecycle as auto-commit DML.
    pub(crate) fn bind_read_operation_storage(&self) -> DBResult<Arc<RwLock<dyn QueryStorage>>> {
        let storage = self.storage.as_ref().ok_or_else(|| {
            DBError::from(QueryError::execution(
                "Read requires a storage binding".to_string(),
            ))
        })?;
        let bound = storage
            .read()
            .bind_read_operation_context()
            .map_err(|error| DBError::from(QueryError::execution(error.to_string())))?;
        Ok(Arc::new(RwLock::new(bound)))
    }

    // ── Cache helpers ──────────────────────────────────────────────────────

    pub(crate) fn invalidate_after_ddl(&self, space_name: Option<&str>) {
        self.schema_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.index_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.optimizer_engine
            .stats_manager()
            .invalidate_space(space_name);
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
        let param_type_signature =
            self.current_param_type_signature(query_text, query_context.request_context());
        self.plan_cache.record_execution_with_space(
            query_text,
            execution_time_ms,
            space_name,
            schema_version,
            index_version,
            param_type_signature,
        );
    }

    fn attach_stream_cache_execution_stats(
        &self,
        stream: &StreamingQueryResult,
        request: &PreparedRequest,
    ) {
        if !is_read_only_cacheable(&request.stmt) {
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
        let param_type_signature = self.current_param_type_signature(
            &request.query_text,
            request.query_context.request_context(),
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
                param_type_signature,
            );
        }));
    }

    /// Hash of the parameter *types* in the current request, matching the
    /// dimension used by the plan-cache put path so execution-time feedback
    /// updates the correct cache entry.
    fn current_param_type_signature(
        &self,
        query_text: &str,
        request: &QueryRequestContext,
    ) -> Option<u64> {
        let mut param_positions = self.param_handler.extract_params(query_text);
        for position in &mut param_positions {
            let name = position
                .name
                .clone()
                .unwrap_or_else(|| position.index.to_string());
            position.expected_type = request.parameters.get(&name).map(|value| value.data_type());
        }
        crate::query::cache::plan_cache::QueryPlanCache::compute_param_type_signature(
            &param_positions,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parser::Parser;

    fn parse(query: &str) -> Stmt {
        let mut parser = Parser::new(query);
        let result = parser
            .parse()
            .unwrap_or_else(|e| panic!("parse failed for {query:?}: {e}"));
        assert!(!parser.has_errors(), "parse errors for {query:?}");
        result.ast.stmt().clone()
    }

    #[test]
    fn all_direct_dml_forms_are_detected() {
        let cases = [
            "INSERT VERTEX person(name) VALUES \"p1\": (\"a\")",
            "DELETE VERTEX \"v1\"",
            "UPDATE VERTEX \"v1\" SET name = \"x\"",
            "MERGE (n:person {name: \"a\"})",
            "SET name = \"x\"",
            "REMOVE v.name",
        ];
        for query in cases {
            let stmt = parse(query);
            assert!(is_direct_dml(&stmt), "direct DML: {query}");
        }
    }

    #[test]
    fn non_direct_dml_forms_are_rejected() {
        let cases = [
            "MATCH (n:person) RETURN n",
            "CREATE TAG IF NOT EXISTS Person(name: STRING, age: INT)",
            "BEGIN",
            "EXPLAIN MATCH (n) RETURN n",
        ];
        for query in cases {
            let stmt = parse(query);
            assert!(!is_direct_dml(&stmt), "not direct DML: {query}");
        }
    }
}
