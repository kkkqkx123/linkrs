//! ChangePasswordExecutor - Change Password Executor
//!
//! Responsible for changing user passwords.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::types::PasswordInfo;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;

/// Change Password Enforcer
///
/// This actuator is responsible for changing user passwords.
#[derive(Debug)]
pub struct ChangePasswordExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    username: Option<String>,
    old_password: String,
    new_password: String,
}

impl<S: StorageClient> ChangePasswordExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        username: Option<String>,
        old_password: String,
        new_password: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "ChangePasswordExecutor".to_string(),
                storage,
                expr_context,
            ),
            username,
            old_password,
            new_password,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for ChangePasswordExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage = storage.write();
        let password_info = PasswordInfo {
            username: self.username.clone(),
            old_password: self.old_password.clone(),
            new_password: self.new_password.clone(),
        };
        let result = storage.change_password(&password_info);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => Err(DBError::storage("Failed to change password")),
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
        "ChangePasswordExecutor"
    }

    fn description(&self) -> &str {
        "Changes a user's password"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for ChangePasswordExecutor<S> {
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
    fn test_change_password_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ChangePasswordExecutor::new(
            1,
            storage,
            Some("test_user".to_string()),
            "old_password".to_string(),
            "new_password".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Execution should succeed") {
            ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ChangePasswordExecutor::new(
            2,
            storage,
            Some("test_user".to_string()),
            "old_password".to_string(),
            "new_password".to_string(),
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
        let executor = ChangePasswordExecutor::new(
            3,
            storage,
            Some("test_user".to_string()),
            "old_password".to_string(),
            "new_password".to_string(),
            expr_context,
        );

        assert_eq!(executor.id(), 3);
        assert_eq!(executor.name(), "ChangePasswordExecutor");
        assert_eq!(executor.description(), "Changes a user's password");
        assert!(executor.stats().num_rows == 0);
    }

    // ==================== Password Handling Unit Tests ====================

    #[test]
    fn test_password_info_creation() {
        let password_info = PasswordInfo {
            username: Some("testuser".to_string()),
            old_password: "oldpass123".to_string(),
            new_password: "newpass456".to_string(),
        };

        assert_eq!(password_info.username, Some("testuser".to_string()));
        assert_eq!(password_info.old_password, "oldpass123");
        assert_eq!(password_info.new_password, "newpass456");
    }

    #[test]
    fn test_password_with_unicode() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ChangePasswordExecutor::new(
            4,
            storage,
            Some("unicode_user".to_string()),
            "中文密码123".to_string(),
            "新密码456".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_password_with_special_chars() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ChangePasswordExecutor::new(
            5,
            storage,
            Some("special_user".to_string()),
            "P@$$w0rd!2024".to_string(),
            "N3w!P@$$w0rd#2024".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_password_none_username() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ChangePasswordExecutor::new(
            6,
            storage,
            None,
            "oldpass".to_string(),
            "newpass".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_accessor_methods() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = ChangePasswordExecutor::new(
            7,
            storage,
            Some("accessor_test".to_string()),
            "old".to_string(),
            "new".to_string(),
            expr_context,
        );

        assert_eq!(executor.id(), 7);
        assert_eq!(executor.name(), "ChangePasswordExecutor");
        assert!(executor.description().contains("password"));
    }
}
