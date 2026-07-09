//! GrantRoleExecutor – The role assignment executor
//!
//! Responsible for granting users role permissions in a specified space.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::RoleType;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

/// Granting the role to the executor
///
/// This executor is responsible for granting users role permissions within the specified space.
#[derive(Debug)]
pub struct GrantRoleExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    username: String,
    space_name: String,
    role: RoleType,
}

impl<S: StorageClient> GrantRoleExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        username: String,
        space_name: String,
        role: RoleType,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "GrantRoleExecutor".to_string(), storage, expr_context),
            username,
            space_name,
            role,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for GrantRoleExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let space_id = storage_guard
            .get_space_id(&self.space_name)
            .map_err(|e| DBError::storage(format!("Failed to get space ID: {}", e)))?;

        let result = storage_guard.grant_role(&self.username, space_id, self.role);

        match result {
            Ok(_) => Ok(ExecutionResult::Success),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to grant role: {}",
                e
            ))),
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
        "GrantRoleExecutor"
    }

    fn description(&self) -> &str {
        "Grants a role to a user in a space"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for GrantRoleExecutor<S> {
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
    fn test_grant_role_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = GrantRoleExecutor::new(
            1,
            storage,
            "test_user".to_string(),
            "test_space".to_string(),
            RoleType::User,
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = GrantRoleExecutor::new(
            2,
            storage,
            "test_user".to_string(),
            "test_space".to_string(),
            RoleType::User,
            expr_context,
        );

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
        let executor = GrantRoleExecutor::new(
            3,
            storage,
            "test_user".to_string(),
            "test_space".to_string(),
            RoleType::User,
            expr_context,
        );

        assert_eq!(executor.id(), 3);
        assert_eq!(executor.name(), "GrantRoleExecutor");
        assert_eq!(executor.description(), "Grants a role to a user in a space");
        assert!(executor.stats().num_rows == 0);
    }
}
