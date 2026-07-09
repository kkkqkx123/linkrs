//! ClearSpaceExecutor - ClearSpaceExecutor
//!
//! Responsible for emptying all data in the specified space.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageSchemaOps;

/// Empty space actuator
///
/// This executor is responsible for emptying all data in the specified space.
#[derive(Debug)]
pub struct ClearSpaceExecutor<S: StorageSchemaOps> {
    base: BaseExecutor<S>,
    space_name: String,
}

impl<S: StorageSchemaOps> ClearSpaceExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "ClearSpaceExecutor".to_string(), storage, expr_context),
            space_name,
        }
    }
}

impl<S: StorageSchemaOps + Send + Sync + 'static> Executor<S> for ClearSpaceExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let result = storage_guard.clear_space(&self.space_name);

        match result {
            Ok(_) => Ok(ExecutionResult::Success),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to clear space: {}",
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
        "ClearSpaceExecutor"
    }

    fn description(&self) -> &str {
        "Clears all data in a space"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageSchemaOps> HasStorage<S> for ClearSpaceExecutor<S> {
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
    fn test_clear_space_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            ClearSpaceExecutor::new(1, storage, "test_space".to_string(), expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            ClearSpaceExecutor::new(2, storage, "test_space".to_string(), expr_context);

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
        let executor = ClearSpaceExecutor::new(3, storage, "test_space".to_string(), expr_context);

        assert_eq!(executor.id(), 3);
        assert_eq!(executor.name(), "ClearSpaceExecutor");
        assert_eq!(executor.description(), "Clears all data in a space");
        assert!(executor.stats().num_rows == 0);
    }
}
