//! CreateTagExecutor - Create Tag Executor
//!
//! Responsible for creating new labels in the specified graph space.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::types::{PropertyDef, TagInfo};
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::{StorageReader, StorageSchemaOps};

pub fn tag_info_from_executor(executor_info: &ExecutorTagInfo) -> TagInfo {
    let properties: Vec<PropertyDef> = executor_info
        .properties
        .iter()
        .map(|p| PropertyDef {
            name: p.name.clone(),
            data_type: p.data_type.clone(),
            nullable: p.nullable,
            default: p.default.clone(),
            comment: p.comment.clone(),
        })
        .collect();

    TagInfo {
        tag_id: 0,
        tag_name: executor_info.tag_name.clone(),
        properties,
        comment: executor_info.comment.clone(),
        ttl_duration: None,
        ttl_col: None,
    }
}

/// Labeling information (for use within actuators)
#[derive(Debug, Clone)]
pub struct ExecutorTagInfo {
    pub space_name: String,
    pub tag_name: String,
    pub properties: Vec<PropertyDef>,
    pub comment: Option<String>,
}

impl ExecutorTagInfo {
    pub fn new(space_name: String, tag_name: String) -> Self {
        Self {
            space_name,
            tag_name,
            properties: Vec::new(),
            comment: None,
        }
    }

    pub fn with_properties(mut self, properties: Vec<PropertyDef>) -> Self {
        self.properties = properties;
        self
    }

    pub fn with_comment(mut self, comment: String) -> Self {
        self.comment = Some(comment);
        self
    }
}

/// Creating a Label Actuator
///
/// This executor is responsible for creating new labels in the specified graph space.
#[derive(Debug)]
pub struct CreateTagExecutor<S: StorageReader + StorageSchemaOps> {
    base: BaseExecutor<S>,
    tag_info: ExecutorTagInfo,
    if_not_exists: bool,
}

impl<S: StorageReader + StorageSchemaOps> CreateTagExecutor<S> {
    /// Create a new CreateTagExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        tag_info: ExecutorTagInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "CreateTagExecutor".to_string(), storage, expr_context),
            tag_info,
            if_not_exists: false,
        }
    }

    /// Creating a CreateTagExecutor with the IF NOT EXISTS option
    pub fn with_if_not_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        tag_info: ExecutorTagInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "CreateTagExecutor".to_string(), storage, expr_context),
            tag_info,
            if_not_exists: true,
        }
    }
}

impl<S: StorageReader + StorageSchemaOps + Send + Sync + 'static> Executor<S>
    for CreateTagExecutor<S>
{
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        if self.if_not_exists {
            let existing =
                storage_guard.get_tag(&self.tag_info.space_name, &self.tag_info.tag_name);
            if let Ok(Some(_)) = existing {
                return Ok(ExecutionResult::Success);
            }
        }

        let metadata_tag_info = tag_info_from_executor(&self.tag_info);
        let result = storage_guard.create_tag(&self.tag_info.space_name, &metadata_tag_info);

        match result {
            Ok(_) => Ok(ExecutionResult::Success),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to create tag: {}",
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
        "CreateTagExecutor"
    }

    fn description(&self) -> &str {
        "Creates a new tag"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader + StorageSchemaOps> crate::query::executor::base::HasStorage<S>
    for CreateTagExecutor<S>
{
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
