//! Explain Executor
//!
//! Executor for EXPLAIN and EXPLAIN ANALYZE statements.
//! Generates query plan descriptions with optional actual execution statistics.

use std::sync::Arc;
use std::time::Instant;

use crate::core::error::DBResult;
use crate::query::core::NodeType;
use crate::query::executor::base::{
    BaseExecutor, DBResult as ExecutorDBResult, ExecutionResult, Executor, ExecutorStats,
};
use crate::query::executor::factory::ExecutorFactory;
use crate::query::parser::ast::stmt::ExplainFormat;
use crate::query::planning::plan::explain::{DescribeVisitor, PlanDescription, ProfilingStats};
use crate::query::planning::plan::ExecutionPlan;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

use super::execution_stats_context::{ExecutionStatsContext, NodeExecutionStats};
use super::format::{format_plan_as_dot, format_plan_with_output_table};
use super::instrumented_executor::InstrumentedExecutor;

/// Explain execution mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExplainMode {
    /// Only show query plan without execution
    PlanOnly,
    /// Execute query and show actual statistics
    Analyze,
}

/// Explain executor
///
/// Handles EXPLAIN and EXPLAIN ANALYZE statements.
/// For EXPLAIN: generates plan description only.
/// For EXPLAIN ANALYZE: executes the query and collects actual statistics.
pub struct ExplainExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    inner_plan: ExecutionPlan,
    format: ExplainFormat,
    mode: ExplainMode,
}

impl<S: StorageClient + Send + 'static> ExplainExecutor<S> {
    pub fn new(
        base: BaseExecutor<S>,
        inner_plan: ExecutionPlan,
        format: ExplainFormat,
        mode: ExplainMode,
    ) -> Self {
        Self {
            base,
            inner_plan,
            format,
            mode,
        }
    }

    /// Generate plan description from execution plan
    fn generate_plan_description(&self) -> DBResult<PlanDescription> {
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

    /// Execute the inner plan with instrumentation
    fn execute_with_instrumentation(
        &mut self,
    ) -> DBResult<(ExecutionResult, Arc<ExecutionStatsContext>)> {
        let stats_context = Arc::new(ExecutionStatsContext::new());

        let exec_result = if let Some(ref root) = self.inner_plan.root {
            let mut factory = ExecutorFactory::with_storage(self.get_storage().clone());
            let context = crate::query::executor::base::ExecutionContext::new(std::sync::Arc::new(
                ExpressionAnalysisContext::new(),
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

        Ok((exec_result, stats_context))
    }

    fn get_storage(&self) -> &Arc<parking_lot::RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not set")
    }

    /// Attach execution statistics to plan description
    fn attach_execution_stats(
        &self,
        plan_desc: &mut PlanDescription,
        node_stats: &std::collections::HashMap<i64, NodeExecutionStats>,
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

    /// Format plan description according to output format
    fn format_output(&self, plan_desc: &PlanDescription) -> DBResult<String> {
        match self.format {
            ExplainFormat::Table => {
                // Use output module's table formatter
                format_plan_with_output_table(plan_desc).map_err(|e| {
                    crate::core::error::DBError::from(crate::core::error::QueryError::execution(
                        e.to_string(),
                    ))
                })
            }
            ExplainFormat::Dot => {
                // Keep existing DOT format (not migrated to output module)
                Ok(format_plan_as_dot(plan_desc))
            }
        }
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for ExplainExecutor<S> {
    fn execute(&mut self) -> ExecutorDBResult<ExecutionResult> {
        match self.mode {
            ExplainMode::PlanOnly => {
                let plan_desc = self.generate_plan_description()?;
                let output = self.format_output(&plan_desc)?;
                let dataset = crate::query::DataSet::from_rows(
                    vec![vec![crate::core::value::Value::String(output)]],
                    vec!["plan".to_string()],
                );
                Ok(ExecutionResult::from_data_set(dataset))
            }

            ExplainMode::Analyze => {
                let start = Instant::now();
                let (_exec_result, stats_context) = self.execute_with_instrumentation()?;
                let execution_time_ms = start.elapsed().as_micros() as f64 / 1000.0;

                let mut plan_desc = self.generate_plan_description()?;
                let node_stats = stats_context.collect_stats();
                self.attach_execution_stats(&mut plan_desc, &node_stats);

                let output = self.format_output(&plan_desc)?;

                let planning_time = stats_context.get_global_stats().planning_time_ms();
                let total_time = planning_time + execution_time_ms;

                let full_output = format!(
                    "{}\nPlanning Time: {:.3} ms\nExecution Time: {:.3} ms\nTotal Time: {:.3} ms",
                    output, planning_time, execution_time_ms, total_time
                );

                let dataset = crate::query::DataSet::from_rows(
                    vec![vec![crate::core::value::Value::String(full_output)]],
                    vec!["plan".to_string()],
                );
                Ok(ExecutionResult::from_data_set(dataset))
            }
        }
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
    use super::*;

    #[test]
    fn test_explain_mode() {
        assert_ne!(ExplainMode::PlanOnly, ExplainMode::Analyze);
    }
}
