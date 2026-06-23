//! Instrumented Executor
//!
//! A wrapper around executors that collects detailed execution statistics.
//! Used by EXPLAIN ANALYZE and PROFILE to gather actual execution data.

use std::sync::Arc;
use std::time::Instant;

use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, ExecutorStats};
use crate::storage::StorageClient;

use super::execution_stats_context::ExecutionStatsContext;

/// Instrumented executor wrapper
///
/// Wraps an executor to collect detailed execution statistics during query execution.
/// This is similar to PostgreSQL's Instrumentation structure.
pub struct InstrumentedExecutor<S: StorageClient + Send + 'static> {
    inner: ExecutorEnum<S>,
    node_id: i64,
    node_name: String,
    stats: ExecutorStats,
    context: Arc<ExecutionStatsContext>,
    first_row_time: Option<Instant>,
    startup_time_us: u64,
}

impl<S: StorageClient + Send + 'static> InstrumentedExecutor<S> {
    pub fn new(
        inner: ExecutorEnum<S>,
        node_id: i64,
        node_name: String,
        context: Arc<ExecutionStatsContext>,
    ) -> Self {
        Self {
            inner,
            node_id,
            node_name,
            stats: ExecutorStats::default(),
            context,
            first_row_time: None,
            startup_time_us: 0,
        }
    }

    fn collect_inner_stats(&mut self) {
        let inner_stats = self.inner.stats();
        self.stats.memory_peak = inner_stats.memory_peak;
        // Cache stats are now handled internally by CacheStats
        // No need to copy cache_hits and cache_misses
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for InstrumentedExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();

        self.context.on_node_start(self.node_id);

        let result = self.inner.execute();

        let elapsed = start.elapsed();
        self.stats.exec_time_us = elapsed.as_micros() as u64;

        if let Ok(exec_result) = &result {
            let row_count = exec_result.count();
            self.stats.num_rows = row_count;

            if self.first_row_time.is_none() && row_count > 0 {
                self.first_row_time = Some(Instant::now());
                self.startup_time_us = self
                    .first_row_time
                    .unwrap()
                    .duration_since(start)
                    .as_micros() as u64;
            }
        }

        self.collect_inner_stats();
        self.context
            .record_startup_time(self.node_id, self.startup_time_us);
        self.context
            .on_node_complete(self.node_id, self.stats.clone());

        result
    }

    fn open(&mut self) -> DBResult<()> {
        self.inner.open()
    }

    fn close(&mut self) -> DBResult<()> {
        self.inner.close()
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    fn id(&self) -> i64 {
        self.node_id
    }

    fn name(&self) -> &str {
        &self.node_name
    }

    fn description(&self) -> &str {
        "InstrumentedExecutor"
    }

    fn stats(&self) -> &ExecutorStats {
        self.inner.stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.inner.stats_mut()
    }
}

/// Factory for creating instrumented executors
pub struct InstrumentedExecutorFactory;

impl InstrumentedExecutorFactory {
    pub fn wrap<S: StorageClient + Send + 'static>(
        executor: ExecutorEnum<S>,
        node_id: i64,
        node_name: String,
        context: Arc<ExecutionStatsContext>,
    ) -> InstrumentedExecutor<S> {
        InstrumentedExecutor::new(executor, node_id, node_name, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::base::StartExecutor;
    use crate::query::validator::context::ExpressionAnalysisContext;

    #[test]
    fn test_instrumented_executor_creation() {
        use crate::storage::MockStorage;

        let ctx = Arc::new(ExecutionStatsContext::new());
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let start_exec = ExecutorEnum::Start(StartExecutor::<MockStorage>::new(1, expr_ctx));

        let instrumented =
            InstrumentedExecutor::new(start_exec, 1, "Start".to_string(), ctx.clone());

        assert_eq!(instrumented.id(), 1);
        assert_eq!(instrumented.name(), "Start");
    }
}
