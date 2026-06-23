//! SwitchSpaceExecutor - SwitchSpaceExecutor
//!
//! Responsible for switching the space of the current session.

use std::sync::Arc;

use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageReader;
use parking_lot::RwLock;

/// Switching space actuators
///
/// This executor is responsible for switching the space of the current session.
/// It returns SpaceSwitched result with the space summary on success.
#[derive(Debug)]
pub struct SwitchSpaceExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    space_name: String,
}

impl<S: StorageReader> SwitchSpaceExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "SwitchSpaceExecutor".to_string(), storage, expr_context),
            space_name,
        }
    }
}

impl<S: StorageReader + Send + Sync + 'static> Executor<S> for SwitchSpaceExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        match storage_guard.get_space(&self.space_name) {
            Ok(Some(space_info)) => {
                let summary = space_info.summary();
                Ok(ExecutionResult::SpaceSwitched(summary))
            }
            Ok(None) => {
                let available_spaces = storage_guard
                    .list_spaces()
                    .unwrap_or_default()
                    .iter()
                    .map(|s| s.space_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");

                let hint = if available_spaces.is_empty() {
                    "No spaces exist. Use 'CREATE SPACE <name>' to create one.".to_string()
                } else {
                    format!(
                        "Available spaces: {}. Use 'CREATE SPACE <name>' to create a new space.",
                        available_spaces
                    )
                };

                Ok(ExecutionResult::Error(format!(
                    "Space '{}' does not exist. {}",
                    self.space_name, hint
                )))
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to get space '{}': {}",
                self.space_name, e
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
        "SwitchSpaceExecutor"
    }

    fn description(&self) -> &str {
        "Switches to a different space"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for SwitchSpaceExecutor<S> {
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
    fn test_switch_space_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            SwitchSpaceExecutor::new(1, storage, "test_space".to_string(), expr_context);

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
            SwitchSpaceExecutor::new(2, storage, "test_space".to_string(), expr_context);

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
        let executor = SwitchSpaceExecutor::new(3, storage, "test_space".to_string(), expr_context);

        assert_eq!(executor.id(), 3);
        assert_eq!(executor.name(), "SwitchSpaceExecutor");
        assert_eq!(executor.description(), "Switches to a different space");
        assert!(executor.stats().num_rows == 0);
    }
}
