//! AlterUserExecutor – Modifier for the user executor
//!
//! Responsible for modifying user attributes (such as role, lock status, etc.).

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::types::UserAlterInfo;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

/// Modify the user executor
///
/// This executor is responsible for modifying user attributes.
#[derive(Debug)]
pub struct AlterUserExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    alter_info: UserAlterInfo,
}

impl<S: StorageClient> AlterUserExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        alter_info: UserAlterInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "AlterUserExecutor".to_string(), storage, expr_context),
            alter_info,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for AlterUserExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage = storage.write();
        let result = storage.alter_user(&self.alter_info);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => Err(DBError::storage("Failed to alter user")),
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
        "AlterUserExecutor"
    }

    fn description(&self) -> &str {
        "Alters a user"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for AlterUserExecutor<S> {
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
    fn test_alter_user_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let alter_info = UserAlterInfo::new("test_user".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = AlterUserExecutor::new(1, storage, alter_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Expected result to exist") {
            ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let alter_info = UserAlterInfo::new("test_user".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = AlterUserExecutor::new(2, storage, alter_info, expr_context);

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
        let alter_info = UserAlterInfo::new("test_user".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = AlterUserExecutor::new(3, storage, alter_info, expr_context);

        assert_eq!(executor.id(), 3);
        assert_eq!(executor.name(), "AlterUserExecutor");
        assert_eq!(executor.description(), "Alters a user");
        assert!(executor.stats().num_rows == 0);
    }
}
