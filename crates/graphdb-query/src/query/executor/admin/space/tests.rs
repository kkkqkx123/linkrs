#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::query::executor::admin::space::create_space::ExecutorSpaceInfo;
    use crate::query::executor::admin::space::{
        CreateSpaceExecutor, DescSpaceExecutor, DropSpaceExecutor, ShowSpacesExecutor,
    };
    use crate::query::executor::Executor;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;

    #[test]
    fn test_create_space_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let space_info =
            ExecutorSpaceInfo::new("test_space".to_string()).with_vid_type("INT64".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = CreateSpaceExecutor::new(1, storage, space_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_drop_space_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            DropSpaceExecutor::new(2, storage, "test_space".to_string(), expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_desc_space_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            DescSpaceExecutor::new(3, storage, "test_space".to_string(), expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_spaces_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ShowSpacesExecutor::new(4, storage, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_space_info_builder() {
        let space_info = ExecutorSpaceInfo::new("my_space".to_string());
        assert_eq!(space_info.space_name, "my_space");
        assert_eq!(space_info.vid_type, "FIXED_STRING(32)");

        let space_info = space_info.with_vid_type("INT32".to_string());

        assert_eq!(space_info.vid_type, "INT32");
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let space_info = ExecutorSpaceInfo::new("test_space".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = CreateSpaceExecutor::new(5, storage, space_info, expr_context);

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
        let space_info = ExecutorSpaceInfo::new("test_space".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = CreateSpaceExecutor::new(6, storage, space_info, expr_context);

        assert_eq!(executor.id(), 6);
        assert_eq!(executor.name(), "CreateSpaceExecutor");
        assert_eq!(executor.description(), "Creates a new graph space");
        assert!(executor.stats().num_rows == 0);
    }
}
