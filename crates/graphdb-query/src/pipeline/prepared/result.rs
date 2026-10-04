use std::sync::Arc;
use std::time::Instant;

use graphdb_core::error::{DBError, DBResult, QueryError};
use graphdb_core::types::TransactionId;

use crate::executor::base::ExecutionResult;
use crate::executor::streaming::instance::ResultSink;
use crate::executor::streaming::StreamingQueryResult;
use crate::storage::QueryStorage;

use super::classify::{is_read_only_cacheable, is_transaction};
use super::{PreparedRequest, StatementClass};
use crate::executor::streaming::transaction_scope::TransactionScope;
use crate::planning::planner::PlannerError;
use crate::planning::statements::clauses::exists_planner;
use crate::QueryContext;

pub enum PreparedOutcome {
    Materialized(ExecutionResult),
    Stream(StreamingQueryResult),
}

impl<S: QueryStorage + 'static> crate::pipeline::QueryPipelineManager<S> {
    /// Compile (or get cached) and execute a prepared request with a
    /// materialized or streaming sink, finalizing auto-bound operation
    /// storage on success/failure.
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

    fn reject_conditional_merge_subqueries(request: &PreparedRequest) -> DBResult<()> {
        let crate::parser::ast::Stmt::Merge(merge_stmt) = &request.stmt else {
            return Ok(());
        };
        let qctx = &request.query_context;
        let check_space_id = qctx.space_id().unwrap_or(1);
        let check_space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names: Vec<String> = Vec::new();
        let map_err = |e: PlannerError| DBError::from(QueryError::pipeline_planning_error(e));
        let pattern_props = match &merge_stmt.pattern {
            crate::parser::ast::Pattern::Node(node_pattern) => node_pattern.properties.as_ref(),
            crate::parser::ast::Pattern::Edge(edge_pattern) => edge_pattern.properties.as_ref(),
            _ => None,
        };
        if let Some(props_expr) = pattern_props {
            if let Some(expr_meta) = props_expr.expression() {
                exists_planner::check_expression_subqueries(
                    expr_meta.inner(),
                    qctx,
                    check_space_id,
                    &check_space_name,
                    &outer_col_names,
                )
                .map_err(map_err)?;
            }
        }
        for set_clause in [&merge_stmt.on_match, &merge_stmt.on_create]
            .into_iter()
            .flatten()
        {
            for assignment in &set_clause.assignments {
                if let Some(expr_meta) = assignment.value.expression() {
                    exists_planner::check_expression_subqueries(
                        expr_meta.inner(),
                        qctx,
                        check_space_id,
                        &check_space_name,
                        &outer_col_names,
                    )
                    .map_err(map_err)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn execute_prepared_inner(
        &mut self,
        request: &PreparedRequest,
        transaction_id: Option<TransactionId>,
        sink: ResultSink,
    ) -> DBResult<PreparedOutcome> {
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
        if let Some(bound) = request.bound_statement.as_ref() {
            if super::super::merge_conditional::is_conditional_node_merge(bound) {
                Self::reject_conditional_merge_subqueries(request)?;
                let result = self.execute_conditional_merge(request)?;
                return Ok(match sink {
                    ResultSink::Materialize => PreparedOutcome::Materialized(result),
                    ResultSink::Stream => {
                        PreparedOutcome::Stream(StreamingQueryResult::from_execution_result(result))
                    }
                    ResultSink::Discard => unreachable!("discard sink is rejected by the caller"),
                });
            }
        }
        if let crate::parser::ast::Stmt::UpdateConfigs(update) = &request.stmt {
            let result = Self::prepare_config_update_intent(request, update)?;
            return Ok(match sink {
                ResultSink::Materialize => PreparedOutcome::Materialized(result),
                ResultSink::Stream => {
                    PreparedOutcome::Stream(StreamingQueryResult::from_execution_result(result))
                }
                ResultSink::Discard => unreachable!("discard sink is rejected by the caller"),
            });
        }
        if let crate::parser::ast::Stmt::ShowConfigs(show) = &request.stmt {
            let result = ExecutionResult::ShowConfigs {
                module: show.module.clone(),
            };
            return Ok(match sink {
                ResultSink::Materialize => PreparedOutcome::Materialized(result),
                ResultSink::Stream => {
                    PreparedOutcome::Stream(StreamingQueryResult::from_execution_result(result))
                }
                ResultSink::Discard => unreachable!("discard sink is rejected by the caller"),
            });
        }
        let stream_ddl =
            sink == ResultSink::Stream && request.statement_class == StatementClass::Ddl;

        let physical_plan = self.compile_or_get_cached(
            &request.query_text,
            request.query_context.clone(),
            request.bound_statement.as_ref(),
            &request.stmt,
            &request.ast,
            request.dml_shape_cacheable,
        )?;
        let scope = if is_transaction(&request.stmt) {
            request.transaction_scope.clone()
        } else {
            transaction_id
                .map(|id| TransactionScope::explicit(id, true))
                .unwrap_or_else(|| request.transaction_scope.clone())
        };

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
            crate::parser::ast::Stmt::Explain(ref explain_stmt) => {
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
            crate::parser::ast::Stmt::Profile(ref profile_stmt) => self.execute_profile(
                profile_stmt,
                request.query_context.clone(),
                request.transaction_scope.clone(),
            ),
            _ => Err(DBError::from(QueryError::execution(
                "Not a diagnostic statement".to_string(),
            ))),
        }
    }

    pub(crate) fn execute_analyze(
        &mut self,
        request: &PreparedRequest,
    ) -> DBResult<ExecutionResult> {
        let space_name = match &request.stmt {
            crate::parser::ast::Stmt::Analyze(analyze) => analyze
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

    fn prepare_config_update_intent(
        request: &PreparedRequest,
        update: &crate::parser::ast::UpdateConfigsStmt,
    ) -> DBResult<ExecutionResult> {
        let expression = update.config_value.get_expression().ok_or_else(|| {
            DBError::from(QueryError::execution(
                "UPDATE CONFIGS value has no evaluable expression".to_string(),
            ))
        })?;
        let request_context = request.query_context.request_context();
        let value = super::super::merge_conditional::eval_const_expression(
            &expression,
            &request_context.parameters,
            &request_context.session_variables,
        )
        .map_err(|error| {
            DBError::from(QueryError::execution(format!(
                "UPDATE CONFIGS value must be a constant expression: {error}"
            )))
        })?;
        Ok(ExecutionResult::ConfigUpdate {
            module: update.module.clone(),
            name: update.config_name.clone(),
            value,
        })
    }

    pub(crate) fn invalidate_after_ddl(&self, space_name: Option<&str>) {
        self.schema_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.optimizer_engine
            .stats_manager()
            .invalidate_space(space_name);
        self.optimizer_engine.invalidate_space_feedback(space_name);
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
        stmt: &crate::parser::ast::Stmt,
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

    fn current_param_type_signature(
        &self,
        query_text: &str,
        request: &crate::QueryRequestContext,
    ) -> Option<u64> {
        let mut param_positions = self.param_handler.extract_params(query_text);
        for position in &mut param_positions {
            let name = position
                .name
                .clone()
                .unwrap_or_else(|| position.index.to_string());
            position.expected_type = request.parameters.get(&name).map(|value| value.data_type());
        }
        crate::cache::plan_cache::QueryPlanCache::compute_param_type_signature(&param_positions)
    }
}
