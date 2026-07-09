//! Create Fulltext Index Executor

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::types::FulltextEngineType;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::parser::ast::{IndexFieldDef, IndexOptions};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::search::engine::EngineType;
use crate::search::error::SearchError;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::StorageClient;

/// Configuration for creating a full-text index
pub struct CreateFulltextIndexConfig {
    /// Index name
    pub index_name: String,
    /// Schema name where the index will be created
    pub schema_name: String,
    /// Fields to be indexed
    pub fields: Vec<IndexFieldDef>,
    /// Type of full-text search engine
    pub engine_type: FulltextEngineType,
    /// Index configuration options
    pub options: IndexOptions,
    /// Whether to skip if index already exists
    pub if_not_exists: bool,
    /// Space ID for the index
    pub space_id: u64,
}

/// Executor for creating full-text indexes
#[derive(Debug)]
pub struct CreateFulltextIndexExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    index_name: String,
    schema_name: String,
    fields: Vec<IndexFieldDef>,
    engine_type: FulltextEngineType,
    options: IndexOptions,
    if_not_exists: bool,
    space_id: u64,
    fulltext_manager: Arc<FulltextIndexManager>,
}

impl<S: StorageClient> CreateFulltextIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        config: CreateFulltextIndexConfig,
        expr_context: Arc<ExpressionAnalysisContext>,
        fulltext_manager: Arc<FulltextIndexManager>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "CreateFulltextIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            index_name: config.index_name,
            schema_name: config.schema_name,
            fields: config.fields,
            engine_type: config.engine_type,
            options: config.options,
            if_not_exists: config.if_not_exists,
            space_id: config.space_id,
            fulltext_manager,
        }
    }

    fn convert_engine_type(_engine_type: FulltextEngineType) -> EngineType {
        EngineType::Bm25
    }
}

impl<S: StorageClient> HasStorage<S> for CreateFulltextIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageClient> Executor<S> for CreateFulltextIndexExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let engine_type = Self::convert_engine_type(self.engine_type);
        let tag_name = &self.schema_name;
        let engine_config = serde_json::to_value(&self.options).map_err(|e| {
            DBError::validation(format!("Failed to serialize fulltext options: {}", e))
        })?;

        for field in &self.fields {
            let result =
                futures::executor::block_on(self.fulltext_manager.create_index_with_engine_config(
                    self.space_id,
                    tag_name,
                    &field.field_name,
                    &self.index_name,
                    Some(engine_type),
                    Some(engine_config.clone()),
                ));

            match result {
                Ok(index_id) => {
                    log::info!(
                        "Created fulltext index '{}' with index_id: {}",
                        self.index_name,
                        index_id
                    );
                }
                Err(SearchError::IndexAlreadyExists(_)) => {
                    if self.if_not_exists {
                        log::warn!(
                            "Fulltext index '{}' already exists, skipping",
                            self.index_name
                        );
                    } else {
                        return Err(DBError::search(format!(
                            "Index already exists: {}",
                            self.index_name
                        )));
                    }
                }
                Err(e) => {
                    return Err(DBError::search(e.to_string()));
                }
            }
        }

        Ok(ExecutionResult::Empty)
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
        "CreateFulltextIndexExecutor"
    }

    fn description(&self) -> &str {
        "Create Fulltext Index Executor"
    }

    fn stats(&self) -> &crate::query::executor::ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::ExecutorStats {
        self.base.stats_mut()
    }
}
