use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

#[derive(Debug)]
pub struct DescribeUserExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    username: String,
}

impl<S: StorageClient> DescribeUserExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        username: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "DescribeUserExecutor".to_string(),
                storage,
                expr_context,
            ),
            username,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for DescribeUserExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage = storage.read();
        if !storage.user_exists(&self.username) {
            return Err(DBError::storage(format!(
                "User {} does not exist",
                self.username
            )));
        }
        Ok(ExecutionResult::Success)
    }

    fn open(&mut self) -> DBResult<()> {
        self.base.open()
    }

    fn close(&mut self) -> DBResult<()> {
        self.base.close()
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id()
    }

    fn name(&self) -> &str {
        self.base.name()
    }

    fn description(&self) -> &str {
        "Describe a database user"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for DescribeUserExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
