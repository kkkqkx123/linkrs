use std::sync::Arc;
use std::time::Instant;

use super::super::base::{BaseExecutor, ExecutorStats};
use crate::core::Value;
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageReader;
use parking_lot::RwLock;

#[derive(Debug)]
pub struct LookupIndexExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    index_name: String,
    index_condition: Option<(String, Value)>,
    scan_forward: bool,
    limit: Option<usize>,
}

impl<S: StorageReader> LookupIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        index_name: String,
        index_condition: Option<(String, Value)>,
        scan_forward: bool,
        limit: Option<usize>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "LookupIndexExecutor".to_string(), storage, expr_context),
            index_name,
            index_condition,
            scan_forward,
            limit,
        }
    }
}

impl<S: StorageReader> Executor<S> for LookupIndexExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(values) => {
                let rows = values.into_iter().map(|v| vec![v]).collect();
                let dataset = DataSet::from_rows(rows, vec!["value".to_string()]);
                Ok(ExecutionResult::DataSet(dataset))
            }
            Err(e) => Err(e),
        }
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        "LookupIndexExecutor"
    }

    fn description(&self) -> &str {
        "Lookup index executor - retrieves vertices using index for LOOKUP statement"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for LookupIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader> LookupIndexExecutor<S> {
    fn do_execute(&mut self) -> DBResult<Vec<Value>> {
        let storage = self.get_storage().read();

        let mut results = Vec::new();

        if let Some((prop_name, prop_value)) = &self.index_condition {
            let scan_results = storage.scan_vertices_by_prop(
                "default",
                &self.index_name,
                prop_name,
                prop_value,
            )?;

            for vertex in scan_results {
                results.push(crate::core::Value::Vertex(Box::new(vertex)));

                if let Some(limit) = self.limit {
                    if results.len() >= limit {
                        break;
                    }
                }
            }
        } else {
            let scan_results = if self.scan_forward {
                storage.scan_vertices_by_tag("default", &self.index_name)?
            } else {
                storage.scan_vertices("default")?
            };

            for vertex in scan_results {
                results.push(crate::core::Value::Vertex(Box::new(vertex)));

                if let Some(limit) = self.limit {
                    if results.len() >= limit {
                        break;
                    }
                }
            }
        }

        Ok(results)
    }
}
