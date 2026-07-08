//! Profile Executor
//!
//! Executor for PROFILE statements.
//! Executes the query and returns detailed performance statistics.

use std::sync::Arc;
use std::time::Instant;

use crate::core::error::DBResult as ExecutorDBResult;
use crate::core::Value;
use crate::query::core::NodeType;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, ExecutorStats};
use crate::query::executor::factory::ExecutorFactory;
use crate::query::parser::ast::stmt::ExplainFormat;
use crate::query::planning::plan::explain::{DescribeVisitor, PlanDescription, ProfilingStats};
use crate::query::planning::plan::ExecutionPlan;
use crate::query::DataSet;
use crate::storage::StorageClient;

use super::execution_stats_context::ExecutionStatsContext;
use super::instrumented_executor::InstrumentedExecutor;

/// Profile executor
///
/// Handles PROFILE statements.
/// Executes the query and returns detailed performance statistics similar to PostgreSQL's EXPLAIN ANALYZE.
pub struct ProfileExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    inner_plan: ExecutionPlan,
    format: ExplainFormat,
}

impl<S: StorageClient + Send + 'static> ProfileExecutor<S> {
    pub fn new(base: BaseExecutor<S>, inner_plan: ExecutionPlan, format: ExplainFormat) -> Self {
        Self {
            base,
            inner_plan,
            format,
        }
    }

    fn get_storage(&self) -> &Arc<parking_lot::RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not set")
    }

    /// Generate plan description from execution plan
    fn generate_plan_description(&self) -> crate::core::error::DBResult<PlanDescription> {
        let mut visitor = DescribeVisitor::new();

        if let Some(ref root) = self.inner_plan.root {
            root.accept(&mut visitor);
        }

        let descriptions = visitor.into_descriptions();
        let mut plan_desc = PlanDescription::new();
        plan_desc.format = format!("{:?}", self.format);

        for desc in descriptions {
            plan_desc.add_node_desc(desc);
        }

        Ok(plan_desc)
    }

    /// Execute the inner plan with full instrumentation
    fn execute_profiled(
        &mut self,
    ) -> ExecutorDBResult<(ExecutionResult, Arc<ExecutionStatsContext>)> {
        let stats_context = Arc::new(ExecutionStatsContext::new());

        let _exec_result = if let Some(ref root) = self.inner_plan.root {
            let mut factory = ExecutorFactory::with_storage(self.get_storage().clone());
            let context = crate::query::executor::base::ExecutionContext::new(std::sync::Arc::new(
                crate::query::validator::context::ExpressionAnalysisContext::new(),
            ));
            let executor = factory
                .create_executor(root, self.get_storage().clone(), &context)
                .map_err(|e| {
                    crate::core::error::DBError::from(crate::core::error::QueryError::execution(
                        e.to_string(),
                    ))
                })?;

            let mut instrumented = InstrumentedExecutor::new(
                executor,
                root.id(),
                format!("{:?}", root.node_type_id()),
                stats_context.clone(),
            );

            instrumented.open()?;
            let result = instrumented.execute()?;
            instrumented.close()?;

            result
        } else {
            ExecutionResult::Empty
        };

        Ok((_exec_result, stats_context))
    }

    /// Attach execution statistics to plan description
    fn attach_execution_stats(
        &self,
        plan_desc: &mut PlanDescription,
        node_stats: &std::collections::HashMap<
            i64,
            super::execution_stats_context::NodeExecutionStats,
        >,
    ) {
        for (node_id, stats) in node_stats {
            if let Some(node_desc) = plan_desc.get_node_desc_mut(*node_id) {
                let profiling = ProfilingStats {
                    rows: stats.actual_rows() as i64,
                    exec_duration_in_us: stats.actual_time_us() as i64,
                    total_duration_in_us: stats.executor_stats.total_time_us as i64,
                    other_stats: {
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            "startup_time_ms".to_string(),
                            format!("{:.3}", stats.startup_time_us as f64 / 1000.0),
                        );
                        map.insert(
                            "memory_used".to_string(),
                            format!("{}", stats.memory_used()),
                        );
                        map.insert(
                            "cache_hit_rate".to_string(),
                            format!("{:.2}%", stats.cache_hit_rate() * 100.0),
                        );
                        map
                    },
                };
                node_desc.add_profile(profiling);
            }
        }
    }

    /// Build result DataSet with profiling information
    fn build_profile_result(
        &self,
        plan_desc: &PlanDescription,
        stats_context: &ExecutionStatsContext,
        _execution_time_ms: f64,
    ) -> DataSet {
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

        let _global_stats = stats_context.get_global_stats();

        DataSet {
            col_names: vec![
                "id".to_string(),
                "name".to_string(),
                "dependencies".to_string(),
                "profiling_data".to_string(),
                "operator_info".to_string(),
            ],
            rows: plan_desc
                .plan_node_descs
                .iter()
                .enumerate()
                .map(|(i, _)| {
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
        }
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for ProfileExecutor<S> {
    fn execute(&mut self) -> ExecutorDBResult<ExecutionResult> {
        let start = Instant::now();
        let (_exec_result, stats_context) = self.execute_profiled()?;
        let execution_time_ms = start.elapsed().as_micros() as f64 / 1000.0;

        let mut plan_desc = self.generate_plan_description()?;
        let node_stats = stats_context.collect_stats();
        self.attach_execution_stats(&mut plan_desc, &node_stats);

        let result_dataset =
            self.build_profile_result(&plan_desc, &stats_context, execution_time_ms);

        Ok(ExecutionResult::DataSet(result_dataset))
    }

    fn open(&mut self) -> ExecutorDBResult<()> {
        self.base.open()
    }

    fn close(&mut self) -> ExecutorDBResult<()> {
        self.base.close()
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_profile_executor_creation() {
        // This is a placeholder test
        // Real tests would require a full storage setup
    }
}
