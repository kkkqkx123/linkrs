//! DescSpaceExecutor - describes the graph space executor
//!
//! Responsible for viewing the details of the specified graph space.

use std::sync::Arc;

use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;

use crate::storage::StorageReader;
use parking_lot::RwLock;

/// Figure space details
#[derive(Debug, Clone)]
pub struct SpaceDesc {
    pub id: i32,
    pub name: String,
    pub vid_type: String,
    pub charset: String,
    pub collate: String,
}

impl SpaceDesc {
    pub fn to_row(&self) -> Vec<Value> {
        vec![
            Value::BigInt(self.id as i64),
            Value::String(self.name.clone()),
            Value::String(self.vid_type.clone()),
            Value::String(self.charset.clone()),
            Value::String(self.collate.clone()),
        ]
    }
}

/// Describe the graph space actuator
///
/// This executor is responsible for returning detailed information about the specified graph space.
#[derive(Debug)]
pub struct DescSpaceExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    space_name: String,
}

impl<S: StorageReader> DescSpaceExecutor<S> {
    /// Creating a new DescSpaceExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DescSpaceExecutor".to_string(), storage, expr_context),
            space_name,
        }
    }
}

impl<S: StorageReader + Send + Sync + 'static> Executor<S> for DescSpaceExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let result = storage_guard.get_space(&self.space_name);

        match result {
            Ok(Some(space_info)) => {
                let rows = vec![vec![
                    Value::String(space_info.space_name.clone()),
                    Value::String(format!("{:?}", space_info.vid_type)),
                    Value::String(space_info.comment.clone().unwrap_or_else(|| "".to_string())),
                ]];

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
            Ok(None) => Ok(ExecutionResult::Error(format!(
                "Space '{}' not found. Use 'SHOW SPACES' to list available spaces.",
                self.space_name
            ))),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to describe space: {}",
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
        "DescSpaceExecutor"
    }

    fn description(&self) -> &str {
        "Describes a graph space"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> crate::query::executor::base::HasStorage<S> for DescSpaceExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
