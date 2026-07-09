//! ShowTagsExecutor - list tags executor
//!
//! Responsible for listing all labels in the specified graph space.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;

use crate::storage::StorageReader;

/// List labeling actuators
///
/// This executor is responsible for returning a list of all labels in the specified graph space.
#[derive(Debug)]
pub struct ShowTagsExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    space_name: String,
}

impl<S: StorageReader> ShowTagsExecutor<S> {
    /// Create a new ShowTagsExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "ShowTagsExecutor".to_string(), storage, expr_context),
            space_name,
        }
    }
}

impl<S: StorageReader + Send + Sync + 'static> Executor<S> for ShowTagsExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let result = storage_guard.list_tags(&self.space_name);

        match result {
            Ok(tag_schemas) => {
                let rows: Vec<Vec<Value>> = tag_schemas
                    .iter()
                    .map(|schema| vec![Value::String(schema.tag_name.clone())])
                    .collect();

                let dataset = DataSet {
                    col_names: vec!["Tag Name".to_string()],
                    rows,
                };
                Ok(ExecutionResult::DataSet(dataset))
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to show tags: {}",
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
        "ShowTagsExecutor"
    }

    fn description(&self) -> &str {
        "Shows all tags"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> crate::query::executor::base::HasStorage<S> for ShowTagsExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
