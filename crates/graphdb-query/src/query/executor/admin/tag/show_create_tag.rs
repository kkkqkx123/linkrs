//! ShowCreateTagExecutor – Show CREATE TAG statement executor
//!
//! Responsible for generating the CREATE TAG statement for a specified tag.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;

use crate::storage::StorageReader;

/// Show CREATE TAG Executor
///
/// This executor is responsible for generating the CREATE TAG statement
/// for the specified tag.
#[derive(Debug)]
pub struct ShowCreateTagExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    space_name: String,
    tag_name: String,
}

impl<S: StorageReader> ShowCreateTagExecutor<S> {
    /// Create a new ShowCreateTagExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        tag_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "ShowCreateTagExecutor".to_string(),
                storage,
                expr_context,
            ),
            space_name,
            tag_name,
        }
    }
}

impl<S: StorageReader + Send + Sync + 'static> Executor<S> for ShowCreateTagExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let result = storage_guard.get_tag(&self.space_name, &self.tag_name);

        match result {
            Ok(Some(tag_schema)) => {
                // Generate CREATE TAG statement
                let mut create_stmt = format!("CREATE TAG `{}`(", tag_schema.tag_name);

                let properties: Vec<String> = tag_schema
                    .properties
                    .iter()
                    .map(|prop| {
                        let mut prop_def = format!("  `{}` {:?}", prop.name, prop.data_type);
                        if !prop.nullable {
                            prop_def.push_str(" NOT NULL");
                        }
                        if let Some(default) = &prop.default {
                            prop_def.push_str(&format!(" DEFAULT {}", default));
                        }
                        if let Some(comment) = &prop.comment {
                            prop_def.push_str(&format!(" COMMENT '{}'", comment));
                        }
                        prop_def
                    })
                    .collect();

                create_stmt.push_str(&properties.join(",\n"));
                create_stmt.push_str("\n)");

                // Add TTL if present
                if let Some(ttl_duration) = tag_schema.ttl_duration {
                    create_stmt.push_str(&format!(" TTL_DURATION = {}", ttl_duration));
                }
                if let Some(ttl_col) = &tag_schema.ttl_col {
                    create_stmt.push_str(&format!(" TTL_COL = '{}'", ttl_col));
                }

                let dataset = DataSet {
                    col_names: vec!["Tag".to_string(), "Create Tag".to_string()],
                    rows: vec![vec![
                        Value::String(tag_schema.tag_name.clone()),
                        Value::String(create_stmt),
                    ]],
                };
                Ok(ExecutionResult::DataSet(dataset))
            }
            Ok(None) => Ok(ExecutionResult::Error(format!(
                "Tag '{}' not found in space '{}'",
                self.tag_name, self.space_name
            ))),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to show create tag: {}",
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
        "ShowCreateTagExecutor"
    }

    fn description(&self) -> &str {
        "Shows CREATE TAG statement for a tag"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> crate::query::executor::base::HasStorage<S> for ShowCreateTagExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
