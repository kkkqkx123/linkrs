//! CreateSpaceExecutor – Creates an executor for working with graph spaces.
//!
//! Responsible for creating new graph spaces (single node).

use std::sync::Arc;

use crate::core::types::{DataType, EngineType, SpaceInfo, SpaceStatus};
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageSchemaOps;
use parking_lot::RwLock;

pub fn space_info_from_executor(executor_info: &ExecutorSpaceInfo) -> SpaceInfo {
    let vid_type = match executor_info.vid_type.as_str() {
        "INT64" | "BIGINT" => DataType::BigInt,
        "INT32" | "INT" | "INTEGER" => DataType::Int,
        "INT16" | "SMALLINT" => DataType::SmallInt,
        _ => DataType::String,
    };

    let engine_type = match executor_info.engine_type.as_str() {
        "MEMORY" => EngineType::Memory,
        _ => EngineType::Redb,
    };

    SpaceInfo {
        space_id: 0,
        space_name: executor_info.space_name.clone(),
        vid_type,
        tags: Vec::new(),
        edge_types: Vec::new(),
        version: crate::core::types::MetadataVersion::default(),
        comment: executor_info.comment.clone(),
        storage_path: None,
        isolation_level: crate::core::types::IsolationLevel::default(),
        partition_num: executor_info.partition_num,
        replica_factor: executor_info.replica_factor,
        engine_type,
        status: SpaceStatus::Online,
    }
}

#[derive(Debug, Clone)]
pub struct ExecutorSpaceInfo {
    pub space_name: String,
    pub vid_type: String,
    pub partition_num: i32,
    pub replica_factor: i32,
    pub engine_type: String,
    pub comment: Option<String>,
}

impl ExecutorSpaceInfo {
    pub fn new(space_name: String) -> Self {
        Self {
            space_name,
            vid_type: "FIXED_STRING(32)".to_string(),
            partition_num: 100,
            replica_factor: 1,
            engine_type: "REDB".to_string(),
            comment: None,
        }
    }

    pub fn with_vid_type(mut self, vid_type: String) -> Self {
        self.vid_type = vid_type;
        self
    }

    pub fn with_partition_num(mut self, partition_num: i32) -> Self {
        self.partition_num = partition_num;
        self
    }

    pub fn with_replica_factor(mut self, replica_factor: i32) -> Self {
        self.replica_factor = replica_factor;
        self
    }

    pub fn with_engine_type(mut self, engine_type: String) -> Self {
        self.engine_type = engine_type;
        self
    }

    pub fn with_comment(mut self, comment: Option<String>) -> Self {
        self.comment = comment;
        self
    }
}

#[derive(Debug)]
pub struct CreateSpaceExecutor<S: StorageSchemaOps> {
    base: BaseExecutor<S>,
    space_info: ExecutorSpaceInfo,
    if_not_exists: bool,
}

impl<S: StorageSchemaOps> CreateSpaceExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_info: ExecutorSpaceInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "CreateSpaceExecutor".to_string(), storage, expr_context),
            space_info,
            if_not_exists: false,
        }
    }

    pub fn with_if_not_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_info: ExecutorSpaceInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "CreateSpaceExecutor".to_string(), storage, expr_context),
            space_info,
            if_not_exists: true,
        }
    }
}

impl<S: StorageSchemaOps + Send + Sync + 'static> Executor<S> for CreateSpaceExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();
        let mut storage_guard = storage.write();

        let mut metadata_space_info = space_info_from_executor(&self.space_info);
        let result = storage_guard.create_space(&mut metadata_space_info);

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => {
                if self.if_not_exists {
                    Ok(ExecutionResult::Success)
                } else {
                    Ok(ExecutionResult::Error(format!(
                        "Space '{}' already exists",
                        self.space_info.space_name
                    )))
                }
            }
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to create space: {}",
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
        "CreateSpaceExecutor"
    }

    fn description(&self) -> &str {
        "Creates a new graph space"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageSchemaOps> crate::query::executor::base::HasStorage<S> for CreateSpaceExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
