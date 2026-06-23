//! DescTagExecutor – Description of the tag executor
//!
//! Responsible for viewing the detailed information of the specified tags.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::types::graph_schema::PropertyType;
use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;

use crate::storage::StorageReader;

/// Tag description information
#[derive(Debug, Clone)]
pub struct TagDesc {
    pub space_name: String,
    pub tag_name: String,
    pub field_id: i32,
    pub field_name: String,
    pub field_type: PropertyType,
    pub nullable: bool,
    pub default_value: Option<Value>,
    pub comment: Option<String>,
}

/// Description of the Tag Executor
///
/// This executor is responsible for returning detailed information about the specified tag.
#[derive(Debug)]
pub struct DescTagExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    space_name: String,
    tag_name: String,
}

impl<S: StorageReader> DescTagExecutor<S> {
    /// Create a new DescTagExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        tag_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DescTagExecutor".to_string(), storage, expr_context),
            space_name,
            tag_name,
        }
    }
}

impl<S: StorageReader + Send + Sync + 'static> Executor<S> for DescTagExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let result = storage_guard.get_tag(&self.space_name, &self.tag_name);

        match result {
            Ok(Some(tag_schema)) => {
                let rows: Vec<Vec<Value>> = tag_schema
                    .properties
                    .iter()
                    .map(|field| {
                        vec![
                            Value::String(field.name.clone()),
                            Value::String(field.data_type.to_string()),
                            Value::Bool(field.nullable),
                            Value::String("".to_string()),
                            Value::String("".to_string()),
                        ]
                    })
                    .collect();

                let dataset = DataSet {
                    col_names: vec![
                        "Field".to_string(),
                        "Type".to_string(),
                        "Nullable".to_string(),
                        "Default".to_string(),
                        "Comment".to_string(),
                    ],
                    rows,
                };
                Ok(ExecutionResult::DataSet(dataset))
            }
            Ok(None) => Ok(ExecutionResult::Error(format!(
                "Tag '{}' not found in space '{}'",
                self.tag_name, self.space_name
            ))),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to describe tag: {}",
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
        "DescTagExecutor"
    }

    fn description(&self) -> &str {
        "Describes a tag"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> crate::query::executor::base::HasStorage<S> for DescTagExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
