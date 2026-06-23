//! DropEdgeExecutor – Executor for deleting edges
//!
//! Responsible for deleting the specified edge type and all its associated data.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

/// Delete the edge type executor.
///
/// This executor is responsible for deleting the specified edge type and all its associated data.
#[derive(Debug)]
pub struct DropEdgeExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    space_name: String,
    edge_name: String,
    if_exists: bool,
}

impl<S: StorageClient> DropEdgeExecutor<S> {
    /// Create a new DropEdgeExecutor.
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        edge_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DropEdgeExecutor".to_string(), storage, expr_context),
            space_name,
            edge_name,
            if_exists: false,
        }
    }

    /// Create a DropEdgeExecutor with the IF EXISTS option
    pub fn with_if_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        edge_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DropEdgeExecutor".to_string(), storage, expr_context),
            space_name,
            edge_name,
            if_exists: true,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for DropEdgeExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let result = storage_guard.drop_edge_type(&self.space_name, &self.edge_name);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => {
                if self.if_exists {
                    Ok(ExecutionResult::Success)
                } else {
                    Ok(ExecutionResult::Error(format!(
                        "Edge type '{}' not found in space '{}'",
                        self.edge_name, self.space_name
                    )))
                }
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to drop edge type: {}",
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
        "DropEdgeExecutor"
    }

    fn description(&self) -> &str {
        "Drops an edge type"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for DropEdgeExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
