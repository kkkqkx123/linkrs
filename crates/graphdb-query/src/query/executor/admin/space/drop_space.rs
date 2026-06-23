//! DropSpaceExecutor – The executor responsible for deleting image spaces.
//!
//! Responsible for deleting the specified graph space and all its data.

use std::sync::Arc;

use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageSchemaOps;
use parking_lot::RwLock;

/// Delete the image space executor.
///
/// This executor is responsible for deleting the specified graph space and all its data.
#[derive(Debug)]
pub struct DropSpaceExecutor<S: StorageSchemaOps> {
    base: BaseExecutor<S>,
    space_name: String,
    if_exists: bool,
}

impl<S: StorageSchemaOps> DropSpaceExecutor<S> {
    /// Create a new DropSpaceExecutor.
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DropSpaceExecutor".to_string(), storage, expr_context),
            space_name,
            if_exists: false,
        }
    }

    /// Create a DropSpaceExecutor with the IF EXISTS option
    pub fn with_if_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DropSpaceExecutor".to_string(), storage, expr_context),
            space_name,
            if_exists: true,
        }
    }
}

impl<S: StorageSchemaOps + Send + Sync + 'static> Executor<S> for DropSpaceExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let result = storage_guard.drop_space(&self.space_name);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => {
                if self.if_exists {
                    Ok(ExecutionResult::Success)
                } else {
                    Ok(ExecutionResult::Error(format!(
                        "Space '{}' not found. Use 'SHOW SPACES' to list available spaces.",
                        self.space_name
                    )))
                }
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to drop space: {}",
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
        "DropSpaceExecutor"
    }

    fn description(&self) -> &str {
        "Drops a graph space"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageSchemaOps> crate::query::executor::base::HasStorage<S> for DropSpaceExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
