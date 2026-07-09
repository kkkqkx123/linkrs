//! ShowSpacesExecutor – Lists the image space executors
//!
//! Responsible for listing all created graph spaces.

use std::sync::Arc;

use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;

use crate::storage::StorageReader;
use parking_lot::RwLock;

/// List the executors that run in the graph space
///
/// This executor is responsible for returning a list of all the created graph spaces.
#[derive(Debug)]
pub struct ShowSpacesExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
}

impl<S: StorageReader> ShowSpacesExecutor<S> {
    /// Create a new ShowSpacesExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "ShowSpacesExecutor".to_string(), storage, expr_context),
        }
    }
}

impl<S: StorageReader + Send + Sync + 'static> Executor<S> for ShowSpacesExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let result = storage_guard.list_spaces();

        match result {
            Ok(spaces) => {
                let rows: Vec<Vec<Value>> = spaces
                    .iter()
                    .map(|space| {
                        vec![
                            Value::String(space.space_name.clone()),
                            Value::String(format!("{:?}", space.vid_type)),
                            Value::String(space.comment.clone().unwrap_or_else(|| "".to_string())),
                        ]
                    })
                    .collect();

                let dataset = DataSet {
                    col_names: vec![
                        "Name".to_string(),
                        "Vid Type".to_string(),
                        "Comment".to_string(),
                    ],
                    rows,
                };
                Ok(ExecutionResult::DataSet(dataset))
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to show spaces: {}",
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
        "ShowSpacesExecutor"
    }

    fn description(&self) -> &str {
        "Shows all graph spaces"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> crate::query::executor::base::HasStorage<S> for ShowSpacesExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
