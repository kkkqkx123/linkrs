#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::core::types::PropertyDef;
    use crate::core::DataType;
    use crate::query::executor::admin::edge::alter_edge::{
        AlterEdgeInfo, AlterEdgeItem, AlterEdgeOp,
    };
    use crate::query::executor::admin::edge::create_edge::ExecutorEdgeInfo;
    use crate::query::executor::admin::edge::{
        AlterEdgeExecutor, CreateEdgeExecutor, DescEdgeExecutor, DropEdgeExecutor,
        ShowEdgesExecutor,
    };
    use crate::query::executor::Executor;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;

    #[test]
    fn test_create_edge_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let properties = vec![
            PropertyDef::new("weight".to_string(), DataType::Double),
            PropertyDef::new("since".to_string(), DataType::BigInt),
        ];
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "knows".to_string(),
            "person".to_string(),
            "person".to_string(),
        )
        .with_properties(properties);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = CreateEdgeExecutor::new(1, storage, edge_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::Success => {}
            _ => panic!("Expected Success result"),
        }
    }

    #[test]
    fn test_create_edge_executor_with_if_not_exists() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "knows".to_string(),
            "person".to_string(),
            "person".to_string(),
        );
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor =
            CreateEdgeExecutor::with_if_not_exists(2, storage, edge_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_alter_edge_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let new_prop = PropertyDef::new("label".to_string(), DataType::String);
        let items = vec![
            AlterEdgeItem::add_property(new_prop),
            AlterEdgeItem::drop_property("old_field".to_string()),
        ];
        let alter_info =
            AlterEdgeInfo::new("test_space".to_string(), "knows".to_string()).with_items(items);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = AlterEdgeExecutor::new(3, storage, alter_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_drop_edge_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DropEdgeExecutor::new(
            4,
            storage,
            "test_space".to_string(),
            "knows".to_string(),
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
    fn test_drop_edge_executor_with_if_exists() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DropEdgeExecutor::with_if_exists(
            5,
            storage,
            "test_space".to_string(),
            "knows".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_desc_edge_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DescEdgeExecutor::new(
            6,
            storage,
            "test_space".to_string(),
            "knows".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_edges_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            ShowEdgesExecutor::new(7, storage, "test_space".to_string(), expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_edge_info_builder() {
        let properties = vec![
            PropertyDef::new("weight".to_string(), DataType::Double),
            PropertyDef::new("since".to_string(), DataType::BigInt),
        ];
        let edge_info = ExecutorEdgeInfo::new(
            "my_space".to_string(),
            "knows".to_string(),
            "person".to_string(),
            "person".to_string(),
        )
        .with_properties(properties)
        .with_comment("Friend relationship".to_string());

        assert_eq!(edge_info.space_name, "my_space");
        assert_eq!(edge_info.edge_name, "knows");
        assert_eq!(edge_info.properties.len(), 2);
        assert_eq!(edge_info.comment, Some("Friend relationship".to_string()));
    }

    #[test]
    fn test_alter_edge_info_builder() {
        let new_prop = PropertyDef::new("label".to_string(), DataType::String);
        let items = vec![
            AlterEdgeItem::add_property(new_prop),
            AlterEdgeItem::drop_property("old_field".to_string()),
        ];
        let alter_info =
            AlterEdgeInfo::new("test_space".to_string(), "knows".to_string()).with_items(items);

        assert_eq!(alter_info.items.len(), 2);
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "knows".to_string(),
            "person".to_string(),
            "person".to_string(),
        );
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = CreateEdgeExecutor::new(8, storage, edge_info, expr_context);

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
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "knows".to_string(),
            "person".to_string(),
            "person".to_string(),
        );
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = CreateEdgeExecutor::new(9, storage, edge_info, expr_context);

        assert_eq!(executor.id(), 9);
        assert_eq!(executor.name(), "CreateEdgeExecutor");
        assert_eq!(executor.description(), "Creates a new edge type");
        assert!(executor.stats().num_rows == 0);
    }

    #[test]
    fn test_create_edge_executor_empty_properties() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "empty_edge".to_string(),
            "person".to_string(),
            "person".to_string(),
        );
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = CreateEdgeExecutor::new(10, storage, edge_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_edge_executor_with_all_data_types() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let properties = vec![
            PropertyDef::new("int_field".to_string(), DataType::BigInt),
            PropertyDef::new("bool_field".to_string(), DataType::Bool),
            PropertyDef::new("double_field".to_string(), DataType::Double),
            PropertyDef::new("string_field".to_string(), DataType::String),
            PropertyDef::new("timestamp_field".to_string(), DataType::Timestamp),
        ];
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "typed_edge".to_string(),
            "person".to_string(),
            "person".to_string(),
        )
        .with_properties(properties);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = CreateEdgeExecutor::new(11, storage, edge_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_edge_executor_with_nullable_properties() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let properties = vec![
            PropertyDef::new("required_field".to_string(), DataType::BigInt),
            PropertyDef::new("optional_field".to_string(), DataType::String),
        ];
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "nullable_edge".to_string(),
            "person".to_string(),
            "person".to_string(),
        )
        .with_properties(properties);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = CreateEdgeExecutor::new(12, storage, edge_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_alter_edge_executor_change_operation() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let new_prop = PropertyDef::new("updated_field".to_string(), DataType::String);
        let items = vec![AlterEdgeItem {
            op: AlterEdgeOp::Change,
            property: Some(new_prop),
            property_name: Some("old_field".to_string()),
        }];
        let alter_info =
            AlterEdgeInfo::new("test_space".to_string(), "knows".to_string()).with_items(items);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = AlterEdgeExecutor::new(13, storage, alter_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_alter_edge_executor_empty_items() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let alter_info = AlterEdgeInfo::new("test_space".to_string(), "knows".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = AlterEdgeExecutor::new(14, storage, alter_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_alter_edge_executor_with_comment() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let new_prop = PropertyDef::new("new_field".to_string(), DataType::BigInt);
        let items = vec![AlterEdgeItem::add_property(new_prop)];
        let alter_info = AlterEdgeInfo::new("test_space".to_string(), "knows".to_string())
            .with_items(items)
            .with_comment("Updated edge type".to_string());
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = AlterEdgeExecutor::new(15, storage, alter_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_drop_edge_executor_nonexistent_edge() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DropEdgeExecutor::new(
            16,
            storage,
            "test_space".to_string(),
            "nonexistent_edge".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
        if let crate::query::executor::base::ExecutionResult::Success =
            result.expect("Failed to execute query")
        {}
    }

    #[test]
    fn test_drop_edge_executor_with_if_exists_nonexistent() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DropEdgeExecutor::with_if_exists(
            17,
            storage,
            "test_space".to_string(),
            "nonexistent_edge".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::Success => {}
            _ => panic!("Expected Success result with IF EXISTS for nonexistent edge"),
        }
    }

    #[test]
    fn test_desc_edge_executor_nonexistent_edge() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = DescEdgeExecutor::new(
            18,
            storage,
            "test_space".to_string(),
            "nonexistent_edge".to_string(),
            expr_context,
        );

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::Error(_) => {}
            _ => panic!("Expected Error result for nonexistent edge"),
        }
    }

    #[test]
    fn test_show_edges_executor_empty_space() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor =
            ShowEdgesExecutor::new(19, storage, "empty_space".to_string(), expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.col_names, vec!["Edge Type".to_string()]);
                assert_eq!(dataset.rows.len(), 0);
            }
            _ => panic!("Expected DataSet result"),
        }
    }

    #[test]
    fn test_executor_edge_info_default_values() {
        let edge_info = ExecutorEdgeInfo::new(
            "default_space".to_string(),
            "default_edge".to_string(),
            "person".to_string(),
            "person".to_string(),
        );

        assert_eq!(edge_info.space_name, "default_space");
        assert_eq!(edge_info.edge_name, "default_edge");
        assert!(edge_info.properties.is_empty());
        assert!(edge_info.comment.is_none());
    }

    #[test]
    fn test_alter_edge_item_add_property() {
        let prop = PropertyDef::new("test_prop".to_string(), DataType::BigInt);
        let item = AlterEdgeItem::add_property(prop.clone());

        assert!(matches!(item.op, AlterEdgeOp::Add));
        assert!(item.property.is_some());
        assert!(item.property_name.is_none());
    }

    #[test]
    fn test_alter_edge_item_drop_property() {
        let item = AlterEdgeItem::drop_property("old_prop".to_string());

        assert!(matches!(item.op, AlterEdgeOp::Drop));
        assert!(item.property.is_none());
        assert!(item.property_name.is_some());
    }

    #[test]
    fn test_alter_edge_info_default_values() {
        let alter_info = AlterEdgeInfo::new("test_space".to_string(), "test_edge".to_string());

        assert_eq!(alter_info.space_name, "test_space");
        assert_eq!(alter_info.edge_name, "test_edge");
        assert!(alter_info.items.is_empty());
        assert!(alter_info.comment.is_none());
    }

    #[test]
    fn test_executor_open_close_multiple_times() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "knows".to_string(),
            "person".to_string(),
            "person".to_string(),
        );
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = CreateEdgeExecutor::new(20, storage, edge_info, expr_context);

        assert!(!executor.is_open());
        assert!(executor.open().is_ok());
        assert!(executor.is_open());
        assert!(executor.open().is_ok());
        assert!(executor.is_open());
        assert!(executor.close().is_ok());
        assert!(!executor.is_open());
        assert!(executor.close().is_ok());
        assert!(!executor.is_open());
    }

    #[test]
    fn test_executor_stats_updates() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "knows".to_string(),
            "person".to_string(),
            "person".to_string(),
        );
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = CreateEdgeExecutor::new(21, storage, edge_info, expr_context);

        let stats = executor.stats();
        assert_eq!(stats.num_rows, 0);

        let stats_mut = executor.stats_mut();
        stats_mut.num_rows = 10;

        let updated_stats = executor.stats();
        assert_eq!(updated_stats.num_rows, 10);
    }

    #[test]
    fn test_create_edge_executor_special_characters_in_name() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let edge_info = ExecutorEdgeInfo::new(
            "test_space".to_string(),
            "edge_with_underscore".to_string(),
            "person".to_string(),
            "person".to_string(),
        );
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = CreateEdgeExecutor::new(22, storage, edge_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_alter_edge_executor_multiple_operations() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let items = vec![
            AlterEdgeItem::add_property(PropertyDef::new("field1".to_string(), DataType::BigInt)),
            AlterEdgeItem::add_property(PropertyDef::new("field2".to_string(), DataType::String)),
            AlterEdgeItem::drop_property("old_field1".to_string()),
            AlterEdgeItem::drop_property("old_field2".to_string()),
        ];
        let alter_info =
            AlterEdgeInfo::new("test_space".to_string(), "knows".to_string()).with_items(items);
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let mut executor = AlterEdgeExecutor::new(23, storage, alter_info, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());
    }

    #[test]
    fn test_executor_description_consistency() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let create_executor = CreateEdgeExecutor::new(
            24,
            storage.clone(),
            ExecutorEdgeInfo::new(
                "s".to_string(),
                "e".to_string(),
                "person".to_string(),
                "person".to_string(),
            ),
            expr_context.clone(),
        );
        let alter_executor = AlterEdgeExecutor::new(
            25,
            storage.clone(),
            AlterEdgeInfo::new("s".to_string(), "e".to_string()),
            expr_context.clone(),
        );
        let drop_executor = DropEdgeExecutor::new(
            26,
            storage.clone(),
            "s".to_string(),
            "e".to_string(),
            expr_context.clone(),
        );
        let desc_executor = DescEdgeExecutor::new(
            27,
            storage.clone(),
            "s".to_string(),
            "e".to_string(),
            expr_context.clone(),
        );
        let show_executor = ShowEdgesExecutor::new(28, storage, "s".to_string(), expr_context);

        assert_eq!(create_executor.name(), "CreateEdgeExecutor");
        assert_eq!(alter_executor.name(), "AlterEdgeExecutor");
        assert_eq!(drop_executor.name(), "DropEdgeExecutor");
        assert_eq!(desc_executor.name(), "DescEdgeExecutor");
        assert_eq!(show_executor.name(), "ShowEdgesExecutor");

        assert!(!create_executor.description().is_empty());
        assert!(!alter_executor.description().is_empty());
        assert!(!drop_executor.description().is_empty());
        assert!(!desc_executor.description().is_empty());
        assert!(!show_executor.description().is_empty());
    }

    #[test]
    fn test_executor_edge_info_builder_chain() {
        let properties = vec![
            PropertyDef::new("prop1".to_string(), DataType::BigInt),
            PropertyDef::new("prop2".to_string(), DataType::String),
        ];
        let edge_info = ExecutorEdgeInfo::new(
            "space1".to_string(),
            "edge1".to_string(),
            "person".to_string(),
            "person".to_string(),
        )
        .with_properties(properties)
        .with_comment("Test edge".to_string());

        assert_eq!(edge_info.space_name, "space1");
        assert_eq!(edge_info.edge_name, "edge1");
        assert_eq!(edge_info.properties.len(), 2);
        assert_eq!(edge_info.comment, Some("Test edge".to_string()));
    }

    #[test]
    fn test_alter_edge_info_builder_chain() {
        let items = vec![AlterEdgeItem::add_property(PropertyDef::new(
            "new_prop".to_string(),
            DataType::BigInt,
        ))];
        let alter_info = AlterEdgeInfo::new("space1".to_string(), "edge1".to_string())
            .with_items(items)
            .with_comment("Alter test".to_string());

        assert_eq!(alter_info.space_name, "space1");
        assert_eq!(alter_info.edge_name, "edge1");
        assert_eq!(alter_info.items.len(), 1);
        assert_eq!(alter_info.comment, Some("Alter test".to_string()));
    }
}
