//! DropUserExecutor - Drop User Executor
//!
//! Responsible for deleting database users.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

/// Delete User Executor
///
/// This actuator is responsible for deleting users.
#[derive(Debug)]
pub struct DropUserExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    username: String,
    if_exists: bool,
}

impl<S: StorageClient> DropUserExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        username: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DropUserExecutor".to_string(), storage, expr_context),
            username,
            if_exists: false,
        }
    }

    pub fn with_if_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        username: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DropUserExecutor".to_string(), storage, expr_context),
            username,
            if_exists: true,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for DropUserExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage = storage.write();
        let result = storage.drop_user(&self.username);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => {
                if self.if_exists {
                    Ok(ExecutionResult::Success)
                } else {
                    Err(DBError::storage("User not found"))
                }
            }
            Err(e) => Err(DBError::from(e)),
        }
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
        self.base.id
    }

    fn name(&self) -> &str {
        "DropUserExecutor"
    }

    fn description(&self) -> &str {
        "Drops a user"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for DropUserExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::Executor;
    use crate::storage::MockStorage;
    use ExpressionAnalysisContext;

    #[test]
    fn test_drop_user_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DropUserExecutor::new(1, storage, "test_user".to_string(), expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Expected result to exist") {
            ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_drop_user_executor_if_exists() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            DropUserExecutor::with_if_exists(2, storage, "test_user".to_string(), expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DropUserExecutor::new(3, storage, "test_user".to_string(), expr_context);

        assert!(!executor.is_open());
        assert!(executor.open().is_ok());
        assert!(executor.is_open());
        assert!(executor.close().is_ok());
        assert!(!executor.is_open());
    }

    #[test]
    fn test_executor_stats() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = DropUserExecutor::new(4, storage, "test_user".to_string(), expr_context);

        assert_eq!(executor.id(), 4);
        assert_eq!(executor.name(), "DropUserExecutor");
        assert_eq!(executor.description(), "Drops a user");
        assert!(executor.stats().num_rows == 0);
    }
}
