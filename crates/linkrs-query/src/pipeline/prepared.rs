use std::sync::Arc;

use linkrs_core::error::DBResult;
use linkrs_core::types::SpaceInfo;
use parking_lot::RwLock;

use crate::executor::streaming::transaction_scope::TransactionScope;
use crate::storage::QueryStorage;

mod classify;
mod result;
mod transaction;

use crate::binder::BoundStatement;
use crate::parser::ast::Stmt;
use crate::QueryContext;
use crate::QueryRequestContext;
pub use classify::{
    build_validated_fallback, classify_statement, is_analyze, is_ddl, is_diagnostic,
    is_read_only_cacheable, is_transaction, requires_write_storage, StatementClass,
};
pub use result::PreparedOutcome;

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
    /// snapshot per statement is leaked, degrading loads to O(n^2).
    pub owns_operation_storage: bool,
    /// Fully resolved bound IR, produced by the Binder.
    pub bound_statement: Option<BoundStatement>,
    /// Cloned AST statement for classification and diagnostic matching.
    pub stmt: Stmt,
    /// The parsed AST, retained for planning.
    pub ast: Arc<crate::parser::ast::stmt::Ast>,
    /// whether the query text is a shape-normalized DML template that may
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
    /// `min_active_snapshot_ts`, degrading bulk loads to O(n^2).
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
                .map_err(|error| {
                    linkrs_core::error::DBError::from(linkrs_core::error::QueryError::execution(
                        error.to_string(),
                    ))
                })?;
        }
        Ok(())
    }
}

impl<S: QueryStorage + 'static> crate::pipeline::QueryPipelineManager<S> {
    /// Parse and bind once, producing a [`PreparedRequest`].
    pub(crate) fn prepare_request(
        &mut self,
        query_text: &str,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<PreparedRequest> {
        let mut parser_result = match rctx.parsed_statement.clone() {
            Some(ast) => crate::parser::parsing::ParserResult { ast },
            None => self.parse_into_context(query_text)?,
        };

        let (effective_query, effective_rctx, dml_shape_cacheable) = if self.dml_shape_cache_enabled
            && classify::is_direct_dml_statement(parser_result.ast.stmt())
            && !rctx
                .parameters
                .iter()
                .any(|(name, _)| name.starts_with(crate::planning::dml_shape::DML_PARAM_PREFIX))
        {
            if let Some(shape) =
                crate::planning::dml_shape::normalize_shape(parser_result.ast.stmt())
            {
                let cached_ast = self.lookup_dml_template(&shape.normalized_text);
                let normalized = match cached_ast {
                    Some(ast) => Some(crate::parser::parsing::ParserResult { ast }),
                    None => match self.parse_into_context(&shape.normalized_text) {
                        Ok(parsed) => {
                            self.dml_template_ast_parse_count
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            self.store_dml_template(
                                shape.normalized_text.clone(),
                                parsed.ast.clone(),
                            );
                            Some(parsed)
                        }
                        Err(_) => {
                            log::warn!(
                                "DML shape template failed to re-parse, falling back to non-cached path: {}",
                                shape.normalized_text
                            );
                            None
                        }
                    },
                };
                match normalized {
                    Some(normalized) => {
                        let mut updated = (*rctx).clone();
                        updated.query = shape.normalized_text.clone();
                        for (index, value) in shape.values.iter().enumerate() {
                            updated.parameters.insert(
                                format!(
                                    "{}{}",
                                    crate::planning::dml_shape::DML_PARAM_PREFIX,
                                    index
                                ),
                                value.clone(),
                            );
                        }
                        parser_result = normalized;
                        (shape.normalized_text, Arc::new(updated), true)
                    }
                    None => (query_text.to_string(), rctx, false),
                }
            } else {
                (query_text.to_string(), rctx, false)
            }
        } else {
            (query_text.to_string(), rctx, false)
        };

        let needs_write = requires_write_storage(parser_result.ast.stmt());
        let is_ddl_statement = is_ddl(parser_result.ast.stmt());
        let is_read_only_statement = !needs_write
            && !is_ddl_statement
            && !is_diagnostic(parser_result.ast.stmt())
            && !is_analyze(parser_result.ast.stmt())
            && !is_transaction(parser_result.ast.stmt());
        let auto_commit_needs_binding = effective_rctx.operation_storage.is_none()
            && effective_rctx.auto_commit
            && (needs_write || is_ddl_statement || is_read_only_statement);

        let (operation_storage, effective_rctx, owns_operation_storage) =
            if auto_commit_needs_binding {
                let storage = if needs_write || is_ddl_statement {
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
        let memo_hit = if dml_shape_cacheable {
            let planning_config = self.dml_planning_config();
            let key = super::DmlPlanMemoKey {
                normalized_text: effective_query.clone(),
                space_name: query_context
                    .space_name()
                    .or_else(|| query_context.request_context().space_name.clone()),
                schema_version: Some(
                    self.schema_generation
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
                index_version: Some(
                    self.index_generation
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
                param_sig: Self::dml_param_signature(&query_context.request_context().parameters),
                optimizer_version: planning_config.optimizer_version,
                planning_config_hash: planning_config.config_hash,
            };
            self.find_dml_plan(&key).is_some()
        } else {
            false
        };
        let bound = if memo_hit {
            self.dml_bind_skipped_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
        } else {
            self.bind_parsed_statement(parser_result.ast, query_context.clone())?
        };
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

    pub(crate) fn prepare_request_with_auto_commit(
        &mut self,
        query_text: &str,
        space_info: Option<SpaceInfo>,
    ) -> DBResult<PreparedRequest> {
        let mut rctx = QueryRequestContext::new(query_text.to_string());
        if let Some(ref name) = space_info.as_ref().map(|s| s.space_name.clone()) {
            rctx.space_name = Some(name.clone());
        }
        self.prepare_request(query_text, Arc::new(rctx), space_info)
    }

    fn finalize_prepare(
        query_text: &str,
        query_context: Arc<QueryContext>,
        ast: Arc<crate::parser::ast::stmt::Ast>,
        operation_storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        owns_operation_storage: bool,
        bound_statement: Option<BoundStatement>,
        dml_shape_cacheable: bool,
    ) -> DBResult<PreparedRequest> {
        let stmt = ast.stmt().clone();
        let statement_class = classify_statement(&stmt);
        let transaction_scope =
            transaction::resolve_transaction_scope(&stmt, query_context.request_context());
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

    pub(crate) fn query_context_for_request(
        &self,
        rctx: Arc<QueryRequestContext>,
        space_info: Option<&SpaceInfo>,
    ) -> Arc<QueryContext> {
        let snapshot_ts = transaction::snapshot_ts_for_request(&rctx);
        let isolation_level = transaction::isolation_level_for_request(&rctx);
        let mut builder = QueryContext::builder(rctx);
        if let Some(ts) = snapshot_ts {
            builder = builder.with_snapshot_ts(ts);
        }
        if let Some(level) = isolation_level {
            builder = builder.with_isolation_level(level);
        }
        builder = builder.with_password_history_depth(self.password_history_depth);
        let mut query_context = builder.build();
        if let Some(space) = space_info {
            query_context.set_space_info(space.clone());
        }
        #[allow(clippy::arc_with_non_send_sync)]
        Arc::new(query_context)
    }

    pub(crate) fn bind_auto_commit_storage(&self) -> DBResult<Arc<RwLock<dyn QueryStorage>>> {
        let storage = self.storage.as_ref().ok_or_else(|| {
            linkrs_core::error::DBError::from(linkrs_core::error::QueryError::execution(
                "DML requires a storage binding".to_string(),
            ))
        })?;
        let bound = storage.read().bind_auto_commit_context().map_err(|error| {
            linkrs_core::error::DBError::from(linkrs_core::error::QueryError::execution(
                error.to_string(),
            ))
        })?;
        Ok(Arc::new(RwLock::new(bound)))
    }

    pub(crate) fn bind_read_operation_storage(&self) -> DBResult<Arc<RwLock<dyn QueryStorage>>> {
        let storage = self.storage.as_ref().ok_or_else(|| {
            linkrs_core::error::DBError::from(linkrs_core::error::QueryError::execution(
                "Read requires a storage binding".to_string(),
            ))
        })?;
        let bound = storage
            .read()
            .bind_read_operation_context()
            .map_err(|error| {
                linkrs_core::error::DBError::from(linkrs_core::error::QueryError::execution(
                    error.to_string(),
                ))
            })?;
        Ok(Arc::new(RwLock::new(bound)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

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
            "DELETE VERTEX person FROM \"v1\"",
            "UPDATE VERTEX \"v1\" SET name = \"x\"",
            "MERGE (n:person {name: \"a\"})",
            "SET name = \"x\"",
            "REMOVE v.name",
        ];
        for query in cases {
            let stmt = parse(query);
            assert!(
                classify::is_direct_dml_statement(&stmt),
                "direct DML: {query}"
            );
            assert!(
                classify::is_direct_write_statement(&stmt),
                "direct write: {query}"
            );
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
            assert!(
                !classify::is_direct_dml_statement(&stmt),
                "not direct DML: {query}"
            );
        }
    }

    #[test]
    fn direct_write_covers_dcl_but_not_shape_candidates() {
        let stmt = parse("CREATE USER alice WITH PASSWORD 'secret'");
        assert!(classify::is_direct_dcl(&stmt));
        assert!(classify::is_direct_write_statement(&stmt));
        assert!(!classify::is_direct_dml_statement(&stmt));
        assert!(crate::planning::dml_shape::normalize_shape(&stmt).is_none());
    }

    #[test]
    fn explicit_transaction_inherits_snapshot_timestamp() {
        let op_ctx = crate::storage::StorageOperationContext::transaction_with_timestamps(
            linkrs_core::types::TransactionId(7),
            42,
            Some(43),
            false,
            false,
        );
        let mut rctx = QueryRequestContext::new("MATCH (n) RETURN n".to_string());
        rctx.auto_commit = false;
        rctx.operation_context = Some(op_ctx);

        assert_eq!(transaction::snapshot_ts_for_request(&rctx), Some(42));
    }

    #[test]
    fn auto_commit_has_no_snapshot_timestamp() {
        let op_ctx = crate::storage::StorageOperationContext::transaction_with_timestamps(
            linkrs_core::types::TransactionId(7),
            42,
            Some(43),
            false,
            true,
        );
        let mut rctx = QueryRequestContext::new("MATCH (n) RETURN n".to_string());
        rctx.auto_commit = true;
        rctx.operation_context = Some(op_ctx);

        assert_eq!(transaction::snapshot_ts_for_request(&rctx), None);
    }

    #[test]
    fn explicit_transaction_without_operation_context_has_no_snapshot() {
        let mut rctx = QueryRequestContext::new("MATCH (n) RETURN n".to_string());
        rctx.auto_commit = false;
        rctx.operation_context = None;

        assert_eq!(transaction::snapshot_ts_for_request(&rctx), None);
    }
}
