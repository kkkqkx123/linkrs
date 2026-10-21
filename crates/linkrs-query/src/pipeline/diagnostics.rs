use super::QueryPipelineManager;
use crate::executor::base::ExecutionResult;
use crate::executor::explain::physical_plan_explain::physical_plan_to_plan_description;
use crate::executor::streaming::instance::{QueryBindings, QueryExecutionInstance, ResultSink};
use crate::executor::streaming::transaction_scope::TransactionScope;
use crate::parser::ast::stmt::{ExplainStmt, ProfileStmt};
use crate::planning::plan::explain::ProfilingStats;
use crate::storage::QueryStorage;
use crate::QueryContext;
use linkrs_core::error::{DBError, DBResult, QueryError};
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::Value;
use std::sync::Arc;

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    /// Compile the statement wrapped by an EXPLAIN / PROFILE.
    ///
    /// Goes through the shared `compile_or_get_cached` path so diagnostic
    /// targets reuse the plan cache (and warm it for the same statement
    /// executed directly).  The full request text is used as the cache key
    /// (the inner statement's source span is not separately extracted).
    fn compile_diagnostic_target(
        &mut self,
        qctx: Arc<QueryContext>,
        inner_stmt: &crate::parser::ast::Stmt,
    ) -> DBResult<Arc<crate::executor::streaming::plan::PhysicalPlan>> {
        let key_text = qctx.request_context().query.clone();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let ast = Arc::new(crate::parser::ast::stmt::Ast::new(
            inner_stmt.clone(),
            expr_ctx,
        ));
        let bound = self
            .bind_parsed_statement(ast.clone(), qctx.clone())?
            .ok_or_else(|| {
                DBError::from(QueryError::execution(
                    "EXPLAIN/PROFILE target statement could not be bound".to_string(),
                ))
            })?;
        self.compile_or_get_cached(&key_text, qctx, Some(&bound), inner_stmt, &ast, false)
    }

    pub fn execute_explain(
        &mut self,
        explain_stmt: &ExplainStmt,
        qctx: Arc<QueryContext>,
    ) -> DBResult<ExecutionResult> {
        let inner_stmt = (*explain_stmt.statement).clone();
        let physical_plan = self.compile_diagnostic_target(qctx, &inner_stmt)?;

        let plan_desc =
            crate::executor::explain::physical_plan_explain::physical_plan_to_plan_description(
                &physical_plan,
            );

        let output =
            crate::executor::explain::format::render_plan(&plan_desc, explain_stmt.format.clone());

        let data_set = linkrs_core::DataSet::from_rows(
            vec![vec![linkrs_core::Value::string(output)]],
            vec!["plan".to_string()],
        );
        Ok(ExecutionResult::DataSet { data: data_set })
    }

    pub fn execute_explain_analyze(
        &mut self,
        explain_stmt: &ExplainStmt,
        qctx: Arc<QueryContext>,
        transaction_scope: TransactionScope,
    ) -> DBResult<ExecutionResult> {
        let physical_plan =
            self.compile_diagnostic_target(qctx.clone(), &explain_stmt.statement)?;

        let exec_ctx = self.build_execution_context(&qctx);
        let mut bindings = QueryBindings::from_context(&exec_ctx, transaction_scope);
        bindings.query_id = exec_ctx.query_id;

        let mut instance = QueryExecutionInstance::instantiate_plan(
            physical_plan.clone(),
            bindings,
            ResultSink::Materialize,
            self.query_registry.clone(),
            self.query_manager.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        let _result = instance
            .execute()
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        let mut plan_desc =
            crate::executor::explain::physical_plan_explain::physical_plan_to_plan_description(
                &physical_plan,
            );
        Self::overlay_execution_profile(&instance, &mut plan_desc, exec_ctx.max_workers);

        let output =
            crate::executor::explain::format::render_plan(&plan_desc, explain_stmt.format.clone());

        let data_set = linkrs_core::DataSet::from_rows(
            vec![vec![linkrs_core::Value::string(output)]],
            vec!["plan".to_string()],
        );
        Ok(ExecutionResult::DataSet { data: data_set })
    }

    pub fn execute_profile(
        &mut self,
        profile_stmt: &ProfileStmt,
        qctx: Arc<QueryContext>,
        transaction_scope: TransactionScope,
    ) -> DBResult<ExecutionResult> {
        let physical_plan =
            self.compile_diagnostic_target(qctx.clone(), &profile_stmt.statement)?;

        let exec_ctx = self.build_execution_context(&qctx);
        let mut bindings = QueryBindings::from_context(&exec_ctx, transaction_scope);
        bindings.query_id = exec_ctx.query_id;

        let mut instance = QueryExecutionInstance::instantiate_plan(
            physical_plan.clone(),
            bindings,
            ResultSink::Materialize,
            self.query_registry.clone(),
            self.query_manager.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        instance
            .execute()
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        let mut plan_desc = physical_plan_to_plan_description(&physical_plan);
        Self::overlay_execution_profile(&instance, &mut plan_desc, exec_ctx.max_workers);

        // DOT renders the same profiled plan as a graph; TABLE expands it
        // into the per-operator profiling rows below. Both go through the
        // single `render_plan` dispatch for the string form; the TABLE
        // DataSet below is the sole PROFILE assembly and reuses the shared
        // spill/parallel/deps helpers so formatting stays single-sourced.
        if profile_stmt.format == crate::parser::ast::stmt::ExplainFormat::Dot {
            let output = crate::executor::explain::format::render_plan(
                &plan_desc,
                profile_stmt.format.clone(),
            );
            let data_set = linkrs_core::DataSet::from_rows(
                vec![vec![linkrs_core::Value::string(output)]],
                vec!["plan".to_string()],
            );
            return Ok(ExecutionResult::DataSet { data: data_set });
        }

        let mut ids = Vec::new();
        let mut names = Vec::new();
        let mut dependencies = Vec::new();
        let mut profiling_data = Vec::new();
        let mut operator_info = Vec::new();

        for node_desc in &plan_desc.plan_node_descs {
            ids.push(Value::BigInt(node_desc.id));
            names.push(Value::string_from_owned(node_desc.name.clone()));

            let deps =
                crate::executor::explain::format::format_deps(node_desc.dependencies.as_ref())
                    .replace(',', ", ");
            dependencies.push(Value::string(if deps == "-" { "" } else { &deps }));

            let profile_str = if let Some(ref profiles) = node_desc.profiles {
                profiles
                    .iter()
                    .map(|p| {
                        let base =
                            format!("rows: {}, exec_time: {}us", p.rows, p.exec_duration_in_us);
                        let spill =
                            crate::executor::explain::format::format_spill_suffix(&p.other_stats);
                        if spill.is_empty() {
                            base
                        } else {
                            let pretty = spill
                                .trim_start_matches(',')
                                .split(',')
                                .map(|part| {
                                    let mut kv = part.splitn(2, ':');
                                    match (kv.next(), kv.next()) {
                                        (Some(k), Some(v)) => format!("{k}: {v}"),
                                        _ => part.to_string(),
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!("{base}, {pretty}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            } else {
                "N/A".to_string()
            };
            profiling_data.push(Value::string(&profile_str));

            let info = node_desc
                .description
                .as_ref()
                .map(|descs| {
                    descs
                        .iter()
                        .map(|p| format!("{}: {}", p.key, p.value))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            operator_info.push(Value::string(&info));
        }

        let parallel_info = if plan_desc.actual_workers > 0 {
            format!(
                "requested_workers: {}, actual_workers: {}, wall_time: {}us, work_time: {}us, buffered_chunks_peak: {}, buffered_bytes_peak: {}",
                plan_desc.requested_workers,
                plan_desc.actual_workers,
                plan_desc.parallel_wall_time_us,
                plan_desc.parallel_work_time_us,
                plan_desc.parallel_buffered_chunks_peak,
                plan_desc.parallel_buffered_bytes_peak,
            )
        } else if !plan_desc.parallel_fallback_reason.is_empty() {
            format!(
                "requested_workers: {}, fallback: {}",
                plan_desc.requested_workers, plan_desc.parallel_fallback_reason,
            )
        } else {
            format!(
                "requested_workers: {}, serial only",
                plan_desc.requested_workers
            )
        };
        let parallel_info = if plan_desc.cbo_notes.is_empty() {
            parallel_info
        } else {
            format!("{}; cbo: {}", parallel_info, plan_desc.cbo_notes.join(", "))
        };
        let parallel_info = if plan_desc.columnar_summary.is_empty() {
            parallel_info
        } else {
            format!("{}; {}", parallel_info, plan_desc.columnar_summary)
        };
        ids.push(Value::BigInt(-1));
        names.push(Value::string("Parallel"));
        dependencies.push(Value::string(""));
        profiling_data.push(Value::string(""));
        operator_info.push(Value::string(&parallel_info));

        let total_rows = plan_desc.plan_node_descs.len() + 1;
        let result_dataset = linkrs_core::DataSet {
            col_names: vec![
                "id".to_string(),
                "name".to_string(),
                "dependencies".to_string(),
                "profiling_data".to_string(),
                "operator_info".to_string(),
            ],
            rows: (0..total_rows)
                .map(|i| {
                    use linkrs_core::value::NullType;
                    vec![
                        ids.get(i)
                            .cloned()
                            .unwrap_or_else(|| Value::Null(NullType::Null)),
                        names
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| Value::Null(NullType::Null)),
                        dependencies
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| Value::Null(NullType::Null)),
                        profiling_data
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| Value::Null(NullType::Null)),
                        operator_info
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| Value::Null(NullType::Null)),
                    ]
                })
                .collect(),
        };
        Ok(ExecutionResult::DataSet {
            data: result_dataset,
        })
    }

    /// Overlay the runtime execution profile onto a plan description.
    ///
    /// Shared by the PROFILE and EXPLAIN ANALYZE paths: per-operator actual
    /// rows / execution time from the runtime profile collector are attached
    /// to the matching physical operator nodes, plus parallel profile data.
    fn overlay_execution_profile(
        instance: &QueryExecutionInstance,
        plan_desc: &mut crate::planning::plan::explain::PlanDescription,
        max_workers: usize,
    ) {
        let ((wall_us, work_us, workers, chunks_peak, bytes_peak), columnar) = {
            let profile = instance.runtime().profile().flush_to_collector();
            for (key, op_profile) in &profile.operators {
                let node_id = key.physical_operator_id.0 as i64;
                if let Some(node_desc) = plan_desc.get_node_desc_mut(node_id) {
                    let profiling = ProfilingStats {
                        rows: op_profile.output_rows as i64,
                        exec_duration_in_us: (op_profile.open_time_us
                            + op_profile.next_time_us
                            + op_profile.close_time_us)
                            as i64,
                        total_duration_in_us: (op_profile.open_time_us
                            + op_profile.next_time_us
                            + op_profile.close_time_us)
                            as i64,
                        other_stats: {
                            let mut map = std::collections::HashMap::new();
                            map.insert(
                                "open_time_us".to_string(),
                                op_profile.open_time_us.to_string(),
                            );
                            map.insert(
                                "next_time_us".to_string(),
                                op_profile.next_time_us.to_string(),
                            );
                            map.insert(
                                "close_time_us".to_string(),
                                op_profile.close_time_us.to_string(),
                            );
                            map.insert(
                                "peak_memory_bytes".to_string(),
                                op_profile.peak_memory_bytes.to_string(),
                            );
                            map.insert(
                                "spilled_bytes".to_string(),
                                op_profile.spilled_bytes.to_string(),
                            );
                            map.insert(
                                "spill_count".to_string(),
                                op_profile.spill_count.to_string(),
                            );
                            map.insert(
                                "spilled_rows".to_string(),
                                op_profile.spilled_rows.to_string(),
                            );
                            map
                        },
                    };
                    node_desc.add_profile(profiling);
                    // Surface actual spill next to the static `spill_threshold`
                    // config pair so PROFILE shows configured vs actual together.
                    if op_profile.spilled_bytes > 0
                        || op_profile.spill_count > 0
                        || op_profile.spilled_rows > 0
                    {
                        node_desc
                            .add_description("spilled_rows", op_profile.spilled_rows.to_string());
                        node_desc
                            .add_description("spilled_bytes", op_profile.spilled_bytes.to_string());
                        node_desc
                            .add_description("spill_count", op_profile.spill_count.to_string());
                    }
                }
            }
            (
                profile.parallel_profile(),
                crate::executor::streaming::runtime::ColumnarStatsSnapshot::from_stats(
                    &instance.runtime().columnar_stats(),
                ),
            )
        };
        plan_desc.requested_workers = max_workers;
        if workers > 0 {
            plan_desc.actual_workers = workers;
        }
        plan_desc.parallel_wall_time_us = wall_us;
        plan_desc.parallel_work_time_us = work_us;
        plan_desc.parallel_buffered_chunks_peak = chunks_peak;
        plan_desc.parallel_buffered_bytes_peak = bytes_peak;
        plan_desc.columnar_summary = columnar.summary();
        if plan_desc.actual_workers == 0
            && plan_desc.requested_workers > 1
            && plan_desc.parallel_fallback_reason.is_empty()
        {
            plan_desc.parallel_fallback_reason =
                "serial fallback (parallelism not activated)".to_string();
        }
    }
}
