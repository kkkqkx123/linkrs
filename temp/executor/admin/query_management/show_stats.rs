//! ShowStatsExecutor - Show Stats Executor
//!
//! Responsible for displaying statistical information about the database.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Type of statistics displayed
#[derive(Debug, Clone)]
pub enum ShowStatsType {
    /// Display storage statistics (number of vertices, edges, spaces, labels, edge types)
    Storage,
    /// Display space statistics (space list)
    Space,
}

/// Display Statistical Actuators
///
/// This actuator is responsible for displaying statistical information about the database.
#[derive(Debug)]
pub struct ShowStatsExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    stats_type: ShowStatsType,
}

impl<S: StorageClient> ShowStatsExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        stats_type: ShowStatsType,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "ShowStatsExecutor".to_string(), storage, expr_context),
            stats_type,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for ShowStatsExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let dataset = match &self.stats_type {
            ShowStatsType::Storage => self.show_storage_stats(&*storage_guard),
            ShowStatsType::Space => self.show_space_stats(&*storage_guard),
        };

        Ok(ExecutionResult::DataSet(dataset))
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
        "ShowStatsExecutor"
    }

    fn description(&self) -> &str {
        "Shows database statistics"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> ShowStatsExecutor<S> {
    fn show_storage_stats(&self, storage: &S) -> DataSet {
        let storage_stats = storage.get_storage_stats();

        let rows = vec![
            vec![
                Value::String("Total Vertices".to_string()),
                Value::BigInt(storage_stats.total_vertices as i64),
            ],
            vec![
                Value::String("Total Edges".to_string()),
                Value::BigInt(storage_stats.total_edges as i64),
            ],
            vec![
                Value::String("Total Spaces".to_string()),
                Value::BigInt(storage_stats.total_spaces as i64),
            ],
            vec![
                Value::String("Total Tags".to_string()),
                Value::BigInt(storage_stats.total_tags as i64),
            ],
            vec![
                Value::String("Total Edge Types".to_string()),
                Value::BigInt(storage_stats.total_edge_types as i64),
            ],
            vec![
                Value::String("Total Size (bytes)".to_string()),
                Value::BigInt(storage_stats.total_size_bytes as i64),
            ],
            vec![
                Value::String("Data Size (bytes)".to_string()),
                Value::BigInt(storage_stats.data_size_bytes as i64),
            ],
            vec![
                Value::String("Index Size (bytes)".to_string()),
                Value::BigInt(storage_stats.index_size_bytes as i64),
            ],
        ];

        DataSet {
            col_names: vec!["Statistic".to_string(), "Value".to_string()],
            rows,
        }
    }

    fn show_space_stats(&self, storage: &S) -> DataSet {
        let spaces = storage.list_spaces().unwrap_or_default();

        let rows: Vec<Vec<Value>> = spaces
            .iter()
            .map(|space| {
                vec![
                    Value::String(space.space_name.clone()),
                    Value::BigInt(space.space_id as i64),
                    Value::BigInt(space.tags.len() as i64),
                    Value::BigInt(space.edge_types.len() as i64),
                ]
            })
            .collect();

        DataSet {
            col_names: vec![
                "Space Name".to_string(),
                "Space ID".to_string(),
                "Tags".to_string(),
                "Edge Types".to_string(),
            ],
            rows,
        }
    }
}

impl<S: StorageClient> HasStorage<S> for ShowStatsExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not available")
    }
}

#[cfg(test)]
mod tests {
    use crate::query::executor::admin::query_management::show_stats::{
        ShowStatsExecutor, ShowStatsType,
    };
    use crate::query::executor::Executor;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;

    #[test]
    fn test_show_stats_executor_storage() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ShowStatsExecutor::new(1, storage, ShowStatsType::Storage, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());

        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::DataSet(dataset) => {
                assert_eq!(
                    dataset.col_names,
                    vec!["Statistic".to_string(), "Value".to_string()]
                );
                assert_eq!(dataset.rows.len(), 8);

                let stats_map: std::collections::HashMap<String, i64> = dataset
                    .rows
                    .iter()
                    .filter_map(|row| {
                        if row.len() >= 2 {
                            if let (
                                crate::core::Value::String(key),
                                crate::core::Value::BigInt(value),
                            ) = (&row[0], &row[1])
                            {
                                Some((key.clone(), *value))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();

                assert_eq!(stats_map.get("Total Vertices"), Some(&0));
                assert_eq!(stats_map.get("Total Edges"), Some(&0));
                assert_eq!(stats_map.get("Total Spaces"), Some(&0));
                assert_eq!(stats_map.get("Total Tags"), Some(&0));
                assert_eq!(stats_map.get("Total Edge Types"), Some(&0));
            }
            _ => panic!("Expected DataSet result"),
        }
    }

    #[test]
    fn test_show_stats_executor_space() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ShowStatsExecutor::new(2, storage, ShowStatsType::Space, expr_context);

        let result = executor.execute();
        assert!(result.is_ok());

        match result.expect("Failed to execute query") {
            crate::query::executor::base::ExecutionResult::DataSet(dataset) => {
                assert_eq!(
                    dataset.col_names,
                    vec![
                        "Space Name".to_string(),
                        "Space ID".to_string(),
                        "Tags".to_string(),
                        "Edge Types".to_string(),
                    ]
                );
            }
            _ => panic!("Expected DataSet result"),
        }
    }

    #[test]
    fn test_executor_lifecycle() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ShowStatsExecutor::new(3, storage, ShowStatsType::Storage, expr_context);

        assert!(!executor.is_open());
        assert!(executor.open().is_ok());
        assert!(executor.is_open());
        assert!(executor.close().is_ok());
        assert!(!executor.is_open());
    }

    #[test]
    fn test_executor_metadata() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor = ShowStatsExecutor::new(4, storage, ShowStatsType::Space, expr_context);

        assert_eq!(executor.id(), 4);
        assert_eq!(executor.name(), "ShowStatsExecutor");
        assert_eq!(executor.description(), "Shows database statistics");
        assert!(executor.stats().num_rows == 0);
    }

    #[test]
    fn test_show_stats_type_storage() {
        let stats_type = ShowStatsType::Storage;
        assert!(matches!(stats_type, ShowStatsType::Storage));
    }

    #[test]
    fn test_show_stats_type_space() {
        let stats_type = ShowStatsType::Space;
        assert!(matches!(stats_type, ShowStatsType::Space));
    }

    #[test]
    fn test_show_stats_type_clone() {
        let stats_type = ShowStatsType::Storage;
        let cloned = stats_type.clone();
        assert!(matches!(cloned, ShowStatsType::Storage));
    }

    #[test]
    fn test_show_stats_type_debug() {
        let stats_type = ShowStatsType::Space;
        let debug_str = format!("{:?}", stats_type);
        assert!(debug_str.contains("Space"));
    }

    #[test]
    fn test_executor_with_different_ids() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let executor1 = ShowStatsExecutor::new(
            10,
            storage.clone(),
            ShowStatsType::Storage,
            expr_context.clone(),
        );
        let executor2 = ShowStatsExecutor::new(
            20,
            storage.clone(),
            ShowStatsType::Space,
            expr_context.clone(),
        );

        assert_eq!(executor1.id(), 10);
        assert_eq!(executor2.id(), 20);
    }

    #[test]
    fn test_executor_stats_mutable() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ShowStatsExecutor::new(5, storage, ShowStatsType::Storage, expr_context);

        let stats = executor.stats();
        assert_eq!(stats.num_rows, 0);

        let stats_mut = executor.stats_mut();
        stats_mut.num_rows = 100;

        assert_eq!(executor.stats().num_rows, 100);
    }

    #[test]
    fn test_multiple_executions() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let mut executor = ShowStatsExecutor::new(6, storage, ShowStatsType::Storage, expr_context);

        let result1 = executor.execute();
        assert!(result1.is_ok());

        let result2 = executor.execute();
        assert!(result2.is_ok());

        match (result1, result2) {
            (
                Ok(crate::query::executor::base::ExecutionResult::DataSet(dataset1)),
                Ok(crate::query::executor::base::ExecutionResult::DataSet(dataset2)),
            ) => {
                assert_eq!(dataset1.col_names, dataset2.col_names);
                assert_eq!(dataset1.rows.len(), dataset2.rows.len());
            }
            _ => panic!("Expected DataSet results"),
        }
    }
}
