//! Rebuild Index Executor
//!
//! Provides reconstruction of tab indexes and side indexes.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

/// Rebuild Tag Indexing Actuator
///
/// This executor is responsible for rebuilding the specified tag index.
#[derive(Debug)]
pub struct RebuildTagIndexExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    space_name: String,
    index_name: String,
}

impl<S: StorageClient> RebuildTagIndexExecutor<S> {
    /// Creating a new RebuildTagIndexExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        index_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "RebuildTagIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            space_name,
            index_name,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for RebuildTagIndexExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let result = storage_guard.rebuild_tag_index(&self.space_name, &self.index_name);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => Ok(ExecutionResult::Error(format!(
                "Index '{}' not found",
                self.index_name
            ))),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to rebuild tag index: {}",
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
        "RebuildTagIndexExecutor"
    }

    fn description(&self) -> &str {
        "Rebuilds a tag index"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for RebuildTagIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

/// Rebuild Edge Indexing Actuator
///
/// This executor is responsible for rebuilding the specified edge index.
#[derive(Debug)]
pub struct RebuildEdgeIndexExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
}

impl<S: StorageClient> RebuildEdgeIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "RebuildEdgeIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for RebuildEdgeIndexExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        Err(DBError::storage("edge indexes are not supported"))
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
        "RebuildEdgeIndexExecutor"
    }

    fn description(&self) -> &str {
        "Rebuilds an edge index"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for RebuildEdgeIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
