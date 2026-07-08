//! ShowEdgesExecutor – Executor for listing edge types
//!
//! Responsible for listing all edge types in the specified graph space.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;

use crate::storage::StorageClient;

/// List of edge type executors
///
/// This executor is responsible for returning a list of all edge types in the specified graph space.
#[derive(Debug)]
pub struct ShowEdgesExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    space_name: String,
}

impl<S: StorageClient> ShowEdgesExecutor<S> {
    /// Create a new ShowEdgesExecutor.
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "ShowEdgesExecutor".to_string(), storage, expr_context),
            space_name,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for ShowEdgesExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let result = storage_guard.list_edge_types(&self.space_name);

        match result {
            Ok(edge_schemas) => {
                let rows: Vec<Vec<Value>> = edge_schemas
                    .iter()
                    .map(|schema| vec![Value::String(schema.edge_type_name.clone())])
                    .collect();

                let dataset = DataSet {
                    col_names: vec!["Edge Type".to_string()],
                    rows,
                };
                Ok(ExecutionResult::DataSet(dataset))
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to show edge types: {}",
                e
            ))),
        }
    }

    fn open(&mut self) -> crate::query::executor::base::DBResult<()> {
        self.base.open()
    }

    fn close(&mut self) -> crate::query::executor::base::DBResult<()> {
        self.base.close()
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        "ShowEdgesExecutor"
    }

    fn description(&self) -> &str {
        "Shows all edge types"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for ShowEdgesExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
