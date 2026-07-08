//! Tag Index Executor
//!
//! Provide functions for creating, deleting, describing, and listing tag indexes.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::types::index::IndexConfig;
use crate::core::types::{Index, IndexField, IndexType};
use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;

use crate::storage::StorageClient;

/// Tag index description information
#[derive(Debug, Clone)]
pub struct TagIndexDesc {
    pub index_id: i32,
    pub index_name: String,
    pub tag_name: String,
    pub fields: Vec<String>,
    pub comment: Option<String>,
}

impl TagIndexDesc {
    pub fn from_metadata(info: &Index) -> Self {
        Self {
            index_id: info.id,
            index_name: info.name.clone(),
            tag_name: info.schema_name.clone(),
            fields: info.properties.clone(),
            comment: info.comment.clone(),
        }
    }
}

impl From<&TagIndexDesc> for Index {
    fn from(desc: &TagIndexDesc) -> Self {
        let fields = desc
            .fields
            .iter()
            .map(|field_name| {
                IndexField::new(
                    field_name.clone(),
                    Value::String("string".to_string()),
                    false,
                )
            })
            .collect();

        Index::new(IndexConfig {
            id: 0,
            name: desc.index_name.clone(),
            space_id: 0,
            schema_name: desc.tag_name.clone(),
            fields,
            properties: desc.fields.clone(),
            index_type: IndexType::TagIndex,
            is_unique: false,
            partial_condition: None,
        })
    }
}

/// Create a Tag Index Executor
#[derive(Debug)]
pub struct CreateTagIndexExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    space_name: String,
    index_info: Index,
    if_not_exists: bool,
}

impl<S: StorageClient> CreateTagIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        index_info: Index,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "CreateTagIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            space_name,
            index_info,
            if_not_exists: false,
        }
    }

    pub fn with_if_not_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        index_info: Index,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "CreateTagIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            space_name,
            index_info,
            if_not_exists: true,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for CreateTagIndexExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let result = storage_guard.create_tag_index(&self.space_name, &self.index_info);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => {
                if self.if_not_exists {
                    Ok(ExecutionResult::Success)
                } else {
                    Ok(ExecutionResult::Error(format!(
                        "Index '{}' already exists",
                        self.index_info.name
                    )))
                }
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to create tag index: {}",
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
        "CreateTagIndexExecutor"
    }
    fn description(&self) -> &str {
        "Creates a tag index"
    }
    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }
    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for ShowTagIndexesExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for CreateTagIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

/// Delete the Tag Index Executor
#[derive(Debug)]
pub struct DropTagIndexExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    space_name: String,
    index_name: String,
    if_exists: bool,
}

impl<S: StorageClient> DropTagIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        index_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "DropTagIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            space_name,
            index_name,
            if_exists: false,
        }
    }

    pub fn with_if_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        index_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "DropTagIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            space_name,
            index_name,
            if_exists: true,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for DropTagIndexExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let result = storage_guard.drop_tag_index(&self.space_name, &self.index_name);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => {
                if self.if_exists {
                    Ok(ExecutionResult::Success)
                } else {
                    Ok(ExecutionResult::Error(format!(
                        "Index '{}' not found",
                        self.index_name
                    )))
                }
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to drop tag index: {}",
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
        "DropTagIndexExecutor"
    }
    fn description(&self) -> &str {
        "Drops a tag index"
    }
    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }
    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for DropTagIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

/// Description of the Tag Index Executor
#[derive(Debug)]
pub struct DescTagIndexExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    space_name: String,
    index_name: String,
}

impl<S: StorageClient> DescTagIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        index_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "DescTagIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            space_name,
            index_name,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for DescTagIndexExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let result = storage_guard.get_tag_index(&self.space_name, &self.index_name);

        match result {
            Ok(Some(desc)) => {
                let desc = TagIndexDesc::from_metadata(&desc);
                let rows = vec![vec![
                    Value::String(desc.index_name),
                    Value::String(desc.tag_name),
                    Value::String(desc.fields.join(", ")),
                    Value::String(desc.comment.unwrap_or_default()),
                ]];

                let dataset = DataSet {
                    col_names: vec![
                        "Index Name".to_string(),
                        "Tag Name".to_string(),
                        "Fields".to_string(),
                        "Comment".to_string(),
                    ],
                    rows,
                };
                Ok(ExecutionResult::DataSet(dataset))
            }
            Ok(None) => Ok(ExecutionResult::Error(format!(
                "Index '{}' not found",
                self.index_name
            ))),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to describe tag index: {}",
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
        "DescTagIndexExecutor"
    }
    fn description(&self) -> &str {
        "Describes a tag index"
    }
    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }
    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for DescTagIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

/// List the tag index executor
#[derive(Debug)]
pub struct ShowTagIndexesExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    space_name: String,
}

impl<S: StorageClient> ShowTagIndexesExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "ShowTagIndexesExecutor".to_string(),
                storage,
                expr_context,
            ),
            space_name,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for ShowTagIndexesExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let result = storage_guard.list_tag_indexes(&self.space_name);

        match result {
            Ok(indexes) => {
                let rows: Vec<Vec<Value>> = indexes
                    .iter()
                    .map(|desc| {
                        let desc = TagIndexDesc::from_metadata(desc);
                        vec![
                            Value::String(desc.index_name.clone()),
                            Value::String(desc.tag_name.clone()),
                            Value::String(desc.fields.join(", ")),
                        ]
                    })
                    .collect();

                let dataset = DataSet {
                    col_names: vec![
                        "Index Name".to_string(),
                        "Tag Name".to_string(),
                        "Fields".to_string(),
                    ],
                    rows,
                };
                Ok(ExecutionResult::DataSet(dataset))
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to show tag indexes: {}",
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
        "ShowTagIndexesExecutor"
    }
    fn description(&self) -> &str {
        "Shows all tag indexes"
    }
    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }
    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}
