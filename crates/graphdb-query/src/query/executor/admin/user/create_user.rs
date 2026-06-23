//! CreateUserExecutor – Creates a user executor.
//!
//! Responsible for creating new database users.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::types::UserInfo;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

/// Create a user executor.
///
/// This executor is responsible for creating new users in the storage layer.
#[derive(Debug)]
pub struct CreateUserExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    user_info: UserInfo,
    if_not_exists: bool,
}

impl<S: StorageClient> CreateUserExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        user_info: UserInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "CreateUserExecutor".to_string(), storage, expr_context),
            user_info,
            if_not_exists: false,
        }
    }

    pub fn with_if_not_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        user_info: UserInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "CreateUserExecutor".to_string(), storage, expr_context),
            user_info,
            if_not_exists: true,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for CreateUserExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage = storage.write();
        let result = storage.create_user(&self.user_info);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => {
                if self.if_not_exists {
                    Ok(ExecutionResult::Success)
                } else {
                    Err(DBError::storage("User already exists"))
                }
            }
            Err(ref e) if e.to_string().contains("already exists") && self.if_not_exists => {
                Ok(ExecutionResult::Success)
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
        "CreateUserExecutor"
    }

    fn description(&self) -> &str {
        "Creates a new user"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for CreateUserExecutor<S> {
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
    fn test_create_user_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let user_info = UserInfo::new("test_user".to_string(), "password123".to_string())
            .expect("Failed to create user info");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = CreateUserExecutor::new(1, storage, user_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Expected result to exist") {
            ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_create_user_executor_if_not_exists() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let user_info = UserInfo::new("test_user".to_string(), "password123".to_string())
            .expect("Failed to create user info");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            CreateUserExecutor::with_if_not_exists(2, storage, user_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let user_info = UserInfo::new("test_user".to_string(), "password123".to_string())
            .expect("Failed to create user info");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = CreateUserExecutor::new(3, storage, user_info, expr_context);

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
        let user_info = UserInfo::new("test_user".to_string(), "password123".to_string())
            .expect("Failed to create user info");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = CreateUserExecutor::new(4, storage, user_info, expr_context);

        assert_eq!(executor.id(), 4);
        assert_eq!(executor.name(), "CreateUserExecutor");
        assert_eq!(executor.description(), "Creates a new user");
        assert!(executor.stats().num_rows == 0);
    }
}
