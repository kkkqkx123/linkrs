//! Indexing Operation Actuator
//!
//! Responsible for creating and deleting indexes

use std::sync::Arc;
use std::time::Instant;

use crate::core::error::DBError;
use crate::core::types::index::IndexConfig;
use crate::core::types::{Index, IndexField};
use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutorStats};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageSchemaOps;
use parking_lot::RwLock;

/// Creating an Indexing Executor
///
/// Responsible for creating indexes in the storage tier
pub struct CreateIndexExecutor<S: StorageSchemaOps> {
    base: BaseExecutor<S>,
    index_name: String,
    index_type: crate::core::types::IndexType,
    properties: Vec<String>,
    tag_name: Option<String>,
}

impl<S: StorageSchemaOps> CreateIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        index_name: String,
        index_type: crate::core::types::IndexType,
        properties: Vec<String>,
        tag_name: Option<String>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "CreateIndexExecutor".to_string(), storage, expr_context),
            index_name,
            index_type,
            properties,
            tag_name,
        }
    }
}

impl<S: StorageSchemaOps> HasStorage<S> for CreateIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageSchemaOps + Send + Sync + 'static> Executor<S> for CreateIndexExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(_) => Ok(ExecutionResult::Empty),
            Err(e) => Err(e),
        }
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        "CreateIndexExecutor"
    }

    fn description(&self) -> &str {
        "Create index executor - creates indexes in storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageSchemaOps + Send + Sync + 'static> CreateIndexExecutor<S> {
    fn do_execute(&mut self) -> DBResult<()> {
        let mut storage = self.get_storage().write();

        let target_name = self
            .tag_name
            .clone()
            .or_else(|| Some(self.index_name.clone()))
            .unwrap_or_default();

        let index_type = self.index_type.clone();
        let fields = self
            .properties
            .iter()
            .map(|prop| IndexField::new(prop.clone(), Value::String("string".to_string()), false))
            .collect();
        let index = Index::new(IndexConfig {
            id: 0,
            name: self.index_name.clone(),
            space_id: 0,
            schema_name: target_name,
            fields,
            properties: self.properties.clone(),
            index_type: index_type.clone(),
            is_unique: false,
            partial_condition: None,
        });

        match index_type {
            crate::core::types::IndexType::TagIndex => {
                storage.create_tag_index("default", &index)?;
            }
            crate::core::types::IndexType::EdgeIndex => {
                return Err(DBError::storage("edge indexes are not supported"));
            }
        }

        Ok(())
    }
}

/// Delete Index Executor
///
/// Responsible for deleting indexes from the storage tier
pub struct DropIndexExecutor<S: StorageSchemaOps> {
    base: BaseExecutor<S>,
    _index_name: String,
}

impl<S: StorageSchemaOps> DropIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        _index_name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DropIndexExecutor".to_string(), storage, expr_context),
            _index_name,
        }
    }
}

impl<S: StorageSchemaOps> HasStorage<S> for DropIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageSchemaOps + Send + Sync + 'static> Executor<S> for DropIndexExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(_) => Ok(ExecutionResult::Empty),
            Err(e) => Err(e),
        }
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        "DropIndexExecutor"
    }

    fn description(&self) -> &str {
        "Drop index executor - drops indexes from storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageSchemaOps + Send + Sync + 'static> DropIndexExecutor<S> {
    fn do_execute(&mut self) -> DBResult<()> {
        let mut storage = self.get_storage().write();

        storage.drop_tag_index("default", &self._index_name)?;

        Ok(())
    }
}
