use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
use crate::core::Value;
use crate::query::executor::base::ExecutionResult;
use crate::query::executor::explain::physical_plan_explain::physical_plan_to_plan_description;
use crate::query::executor::streaming::instance::{
    QueryBindings, QueryExecutionInstance, ResultSink,
};
use crate::query::executor::streaming::transaction_scope::TransactionScope;
use crate::query::parser::ast::stmt::{ExplainStmt, ProfileStmt};
use crate::query::planning::plan::explain::ProfilingStats;
use crate::query::validator::ValidatedStatement;
use crate::query::QueryContext;
use crate::storage::QueryStorage;
use std::sync::Arc;

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub fn execute_explain(
        &mut self,
        explain_stmt: &ExplainStmt,
        qctx: Arc<QueryContext>,
    ) -> DBResult<ExecutionResult> {
        let inner_ast = &explain_stmt.statement;
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let validation_info = self.validate_query_with_context(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx.clone(),
            )),
            qctx.clone(),
        )?;
        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx,
            )),
            validation_info,
        );

        let (physical_plan, _optimized_plan) = self.compile(qctx.clone(), &inner_validated)?;

        let plan_desc =
            crate::query::executor::explain::physical_plan_explain::physical_plan_to_plan_description(
                &physical_plan,
            );

        let output = match explain_stmt.format {
            crate::query::parser::ast::stmt::ExplainFormat::Table => {
                crate::query::executor::explain::format::format_plan_as_table(&plan_desc)
            }
            crate::query::parser::ast::stmt::ExplainFormat::Dot => {
                crate::query::executor::explain::format::format_plan_as_dot(&plan_desc)
            }
        };

        let data_set = crate::core::DataSet::from_rows(
            vec![vec![crate::core::Value::String(output)]],
            vec!["plan".to_string()],
        );
        Ok(ExecutionResult::DataSet { data: data_set })
    }

    pub fn execute_explain_analyze(
        &mut self,
        explain_stmt: &ExplainStmt,
        qctx: Arc<QueryContext>,
    ) -> DBResult<ExecutionResult> {
        let inner_ast = &explain_stmt.statement;
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let validation_info = self.validate_query_with_context(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx.clone(),
            )),
            qctx.clone(),
        )?;
        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx,
            )),
            validation_info,
        );

        let (physical_plan, _optimized_plan) = self.compile(qctx.clone(), &inner_validated)?;

        let exec_ctx = self.build_execution_context(&qctx);
        let mut bindings = QueryBindings::from_context(&exec_ctx, TransactionScope::None);
        bindings.query_id = exec_ctx.query_id;

        let mut instance = QueryExecutionInstance::instantiate_plan(
            physical_plan.clone(),
            bindings,
            ResultSink::Materialize,
            self.query_registry.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        let _result = instance
            .execute()
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        let plan_desc =
            crate::query::executor::explain::physical_plan_explain::physical_plan_to_plan_description(
                &physical_plan,
            );

        let output = match explain_stmt.format {
            crate::query::parser::ast::stmt::ExplainFormat::Table => {
                crate::query::executor::explain::format::format_plan_as_table(&plan_desc)
            }
            crate::query::parser::ast::stmt::ExplainFormat::Dot => {
                crate::query::executor::explain::format::format_plan_as_dot(&plan_desc)
            }
        };

        let data_set = crate::core::DataSet::from_rows(
            vec![vec![crate::core::Value::String(output)]],
            vec!["plan".to_string()],
        );
        Ok(ExecutionResult::DataSet { data: data_set })
    }

    pub fn execute_profile(
        &mut self,
        profile_stmt: &ProfileStmt,
        qctx: Arc<QueryContext>,
    ) -> DBResult<ExecutionResult> {
        let inner_ast = &profile_stmt.statement;
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let validation_info = self.validate_query_with_context(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx.clone(),
            )),
            qctx.clone(),
        )?;
        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (**inner_ast).clone(),
                expr_ctx,
            )),
            validation_info,
        );

        let query_text = qctx.request_context().query.clone();
        let physical_plan =
            self.compile_or_get_cached(&query_text, qctx.clone(), &inner_validated)?;

        let exec_ctx = self.build_execution_context(&qctx);
        let mut bindings = QueryBindings::from_context(&exec_ctx, TransactionScope::None);
        bindings.query_id = exec_ctx.query_id;

        let mut instance = QueryExecutionInstance::instantiate_plan(
            physical_plan.clone(),
            bindings,
            ResultSink::Materialize,
            self.query_registry.clone(),
        )
        .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        instance
            .execute()
            .map_err(|e| DBError::from(QueryError::execution(e.to_string())))?;

        let mut plan_desc = physical_plan_to_plan_description(&physical_plan);

        let (wall_us, work_us, workers, chunks_peak, bytes_peak) = {
            let profile = instance.runtime().profile().lock();
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
                            map
                        },
                    };
                    node_desc.add_profile(profiling);
                }
            }
            profile.parallel_profile()
        };
        plan_desc.requested_workers = exec_ctx.max_workers;
        if workers > 0 {
            plan_desc.actual_workers = workers;
        }
        plan_desc.parallel_wall_time_us = wall_us;
        plan_desc.parallel_work_time_us = work_us;
        plan_desc.parallel_buffered_chunks_peak = chunks_peak;
        plan_desc.parallel_buffered_bytes_peak = bytes_peak;
        if plan_desc.actual_workers == 0 && plan_desc.requested_workers > 1 {
            plan_desc.parallel_fallback_reason = "serial fallback (P8 not activated)".to_string();
        }

        let mut ids = Vec::new();
        let mut names = Vec::new();
        let mut dependencies = Vec::new();
        let mut profiling_data = Vec::new();
        let mut operator_info = Vec::new();

        for node_desc in &plan_desc.plan_node_descs {
            ids.push(Value::BigInt(node_desc.id));
            names.push(Value::String(node_desc.name.clone()));

            let deps = node_desc
                .dependencies
                .as_ref()
                .map(|d| {
                    d.iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            dependencies.push(Value::String(deps));

            let profile_str = if let Some(ref profiles) = node_desc.profiles {
                profiles
                    .iter()
                    .map(|p| format!("rows: {}, exec_time: {}us", p.rows, p.exec_duration_in_us))
                    .collect::<Vec<_>>()
                    .join("; ")
            } else {
                "N/A".to_string()
            };
            profiling_data.push(Value::String(profile_str));

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
            operator_info.push(Value::String(info));
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
        ids.push(Value::BigInt(-1));
        names.push(Value::String("Parallel".to_string()));
        dependencies.push(Value::String(String::new()));
        profiling_data.push(Value::String(String::new()));
        operator_info.push(Value::String(parallel_info));

        let total_rows = plan_desc.plan_node_descs.len() + 1;
        let result_dataset = crate::core::DataSet {
            col_names: vec![
                "id".to_string(),
                "name".to_string(),
                "dependencies".to_string(),
                "profiling_data".to_string(),
                "operator_info".to_string(),
            ],
            rows: (0..total_rows)
                .map(|i| {
                    use crate::core::value::NullType;
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
}
