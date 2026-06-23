#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::core::types::PropertyDef;
    use crate::core::DataType;
    use crate::query::executor::admin::tag::alter_tag::{AlterTagInfo, AlterTagItem};
    use crate::query::executor::admin::tag::create_tag::ExecutorTagInfo;
    use crate::query::executor::admin::tag::{
        AlterTagExecutor, CreateTagExecutor, DescTagExecutor, DropTagExecutor, ShowTagsExecutor,
    };
    use crate::query::executor::Executor;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;

    #[test]
    fn test_create_tag_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let properties = vec![
            PropertyDef::new("name".to_string(), DataType::String),
            PropertyDef::new("age".to_string(), DataType::Int),
        ];
        let tag_info = ExecutorTagInfo::new("test_space".to_string(), "person".to_string())
            .with_properties(properties);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = CreateTagExecutor::new(1, storage, tag_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_create_tag_executor_with_if_not_exists() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let tag_info = ExecutorTagInfo::new("test_space".to_string(), "person".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor =
            CreateTagExecutor::with_if_not_exists(2, storage, tag_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_alter_tag_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let new_prop = PropertyDef::new("email".to_string(), DataType::String);
        let items = vec![
            AlterTagItem::add_property(new_prop),
            AlterTagItem::drop_property("old_field".to_string()),
        ];
        let alter_info =
            AlterTagInfo::new("test_space".to_string(), "person".to_string()).with_items(items);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = AlterTagExecutor::new(3, storage, alter_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_drop_tag_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DropTagExecutor::new(
            4,
            storage,
            "test_space".to_string(),
            "person".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_drop_tag_executor_with_if_exists() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DropTagExecutor::with_if_exists(
            5,
            storage,
            "test_space".to_string(),
            "person".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_desc_tag_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DescTagExecutor::new(
            6,
            storage,
            "test_space".to_string(),
            "person".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_tags_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            ShowTagsExecutor::new(7, storage, "test_space".to_string(), expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_tag_info_builder() {
        let properties = vec![
            PropertyDef::new("name".to_string(), DataType::String),
            PropertyDef::new("age".to_string(), DataType::Int),
        ];
        let tag_info = ExecutorTagInfo::new("my_space".to_string(), "person".to_string())
            .with_properties(properties)
            .with_comment("Person tag".to_string());

        assert_eq!(tag_info.space_name, "my_space");
        assert_eq!(tag_info.tag_name, "person");
        assert_eq!(tag_info.properties.len(), 2);
        assert_eq!(tag_info.comment, Some("Person tag".to_string()));
    }

    #[test]
    fn test_alter_tag_info_builder() {
        let new_prop = PropertyDef::new("email".to_string(), DataType::String);
        let items = vec![
            AlterTagItem::add_property(new_prop),
            AlterTagItem::drop_property("old_field".to_string()),
        ];
        let alter_info =
            AlterTagInfo::new("test_space".to_string(), "person".to_string()).with_items(items);

        assert_eq!(alter_info.items.len(), 2);
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let tag_info = ExecutorTagInfo::new("test_space".to_string(), "person".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = CreateTagExecutor::new(8, storage, tag_info, expr_context);

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
        let tag_info = ExecutorTagInfo::new("test_space".to_string(), "person".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = CreateTagExecutor::new(9, storage, tag_info, expr_context);

        assert_eq!(executor.id(), 9);
        assert_eq!(executor.name(), "CreateTagExecutor");
        assert_eq!(executor.description(), "Creates a new tag");
        assert!(executor.stats().num_rows == 0);
    }
}
