//! Describe Fulltext Index Executor

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::StorageClient;

/// Executor for describing full-text index metadata
#[derive(Debug)]
pub struct DescribeFulltextIndexExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    index_name: String,
    space_id: u64,
    fulltext_manager: Arc<FulltextIndexManager>,
}

impl<S: StorageClient> DescribeFulltextIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        index_name: String,
        space_id: u64,
        expr_context: Arc<ExpressionAnalysisContext>,
        fulltext_manager: Arc<FulltextIndexManager>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "DescribeFulltextIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            index_name,
            space_id,
            fulltext_manager,
        }
    }

    fn parse_index_name(&self) -> Option<(String, String)> {
        let parts: Vec<&str> = self.index_name.split('_').collect();
        if parts.len() >= 3 {
            let tag_name = parts[1].to_string();
            let field_name = parts[2..].join("_");
            Some((tag_name, field_name))
        } else {
            None
        }
    }
}

impl<S: StorageClient> HasStorage<S> for DescribeFulltextIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageClient> Executor<S> for DescribeFulltextIndexExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let parsed = self.parse_index_name();

        let (tag_name, field_name) = match parsed {
            Some((t, f)) => (t, f),
            None => {
                return Err(DBError::query(format!(
                    "Invalid fulltext index name format: '{}'. Expected format: <prefix>_<tag>_<field>",
                    self.index_name
                )));
            }
        };

        let engine = self
            .fulltext_manager
            .get_engine(self.space_id, &tag_name, &field_name);

        let metadata = match engine {
            Some(engine) => {
                let stats = futures::executor::block_on(engine.stats())?;
                Some(("Active".to_string(), stats))
            }
            None => None,
        };

        let col_names = vec!["Property".to_string(), "Value".to_string()];

        let rows: Vec<Vec<Value>> = if let Some((status, stats)) = metadata {
            vec![
                vec![
                    Value::String("Index Name".to_string()),
                    Value::String(self.index_name.clone()),
                ],
                vec![
                    Value::String("Space ID".to_string()),
                    Value::BigInt(self.space_id as i64),
                ],
                vec![
                    Value::String("Tag Name".to_string()),
                    Value::String(tag_name),
                ],
                vec![
                    Value::String("Field Name".to_string()),
                    Value::String(field_name),
                ],
                vec![Value::String("Status".to_string()), Value::String(status)],
                vec![
                    Value::String("Document Count".to_string()),
                    Value::BigInt(stats.doc_count as i64),
                ],
                vec![
                    Value::String("Index Size".to_string()),
                    Value::String(format!("{} bytes", stats.index_size)),
                ],
            ]
        } else {
            vec![
                vec![
                    Value::String("Index Name".to_string()),
                    Value::String(self.index_name.clone()),
                ],
                vec![
                    Value::String("Space ID".to_string()),
                    Value::BigInt(self.space_id as i64),
                ],
                vec![
                    Value::String("Tag Name".to_string()),
                    Value::String(tag_name),
                ],
                vec![
                    Value::String("Field Name".to_string()),
                    Value::String(field_name),
                ],
                vec![
                    Value::String("Status".to_string()),
                    Value::String("Not Found".to_string()),
                ],
            ]
        };

        let dataset = DataSet { col_names, rows };
        Ok(ExecutionResult::DataSet(dataset))
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
        self.base.id()
    }

    fn name(&self) -> &str {
        "DescribeFulltextIndexExecutor"
    }

    fn description(&self) -> &str {
        "Executor for describing full-text index metadata"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.stats_mut()
    }
}
