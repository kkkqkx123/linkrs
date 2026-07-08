//! AlterSpaceExecutor – The executor for modifying spaces
//!
//! Responsible for modifying the configuration of the graphic space.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::{StorageReader, StorageSchemaOps};

/// Space modification options
#[derive(Debug, Clone)]
pub enum SpaceAlterOption {
    Comment(String),
}

/// Modified Space Executor
///
/// This actuator is responsible for modifying the configuration of the graphical space.
#[derive(Debug)]
pub struct AlterSpaceExecutor<S: StorageReader + StorageSchemaOps> {
    base: BaseExecutor<S>,
    space_name: String,
    options: Vec<SpaceAlterOption>,
}

impl<S: StorageReader + StorageSchemaOps> AlterSpaceExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        options: Vec<SpaceAlterOption>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "AlterSpaceExecutor".to_string(), storage, expr_context),
            space_name,
            options,
        }
    }
}

impl<S: StorageReader + StorageSchemaOps + Send + Sync + 'static> Executor<S>
    for AlterSpaceExecutor<S>
{
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let space_id = storage_guard
            .get_space_id(&self.space_name)
            .map_err(|e| DBError::storage(format!("Failed to get space ID: {}", e)))?;

        for option in &self.options {
            match option {
                SpaceAlterOption::Comment(comment) => {
                    if let Err(e) = storage_guard.alter_space_comment(space_id, comment.clone()) {
                        return Ok(ExecutionResult::Error(format!(
                            "Failed to alter comment: {}",
                            e
                        )));
                    }
                }
            }
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
        self.base.id
    }

    fn name(&self) -> &str {
        "AlterSpaceExecutor"
    }

    fn description(&self) -> &str {
        "Alters a space's configuration"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader + StorageSchemaOps> HasStorage<S> for AlterSpaceExecutor<S> {
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
    fn test_alter_space_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let options = vec![SpaceAlterOption::Comment("test".to_string())];
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            AlterSpaceExecutor::new(1, storage, "test_space".to_string(), options, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let options = vec![SpaceAlterOption::Comment("test".to_string())];
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            AlterSpaceExecutor::new(2, storage, "test_space".to_string(), options, expr_context);

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
        let options = vec![SpaceAlterOption::Comment("test".to_string())];
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor =
            AlterSpaceExecutor::new(3, storage, "test_space".to_string(), options, expr_context);

        assert_eq!(executor.id(), 3);
        assert_eq!(executor.name(), "AlterSpaceExecutor");
        assert_eq!(executor.description(), "Alters a space's configuration");
        assert!(executor.stats().num_rows == 0);
    }
}
