//! Drop Fulltext Index Executor

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::search::error::SearchError;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::StorageClient;

/// Executor for dropping full-text indexes
#[derive(Debug)]
pub struct DropFulltextIndexExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    index_name: String,
    if_exists: bool,
    space_id: u64,
    fulltext_manager: Arc<FulltextIndexManager>,
}

impl<S: StorageClient> DropFulltextIndexExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        index_name: String,
        if_exists: bool,
        space_id: u64,
        expr_context: Arc<ExpressionAnalysisContext>,
        fulltext_manager: Arc<FulltextIndexManager>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "DropFulltextIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            index_name,
            if_exists,
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

impl<S: StorageClient> HasStorage<S> for DropFulltextIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageClient> Executor<S> for DropFulltextIndexExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let parsed = self.parse_index_name();

        match parsed {
            Some((tag_name, field_name)) => {
                let result = futures::executor::block_on(self.fulltext_manager.drop_index(
                    self.space_id,
                    &tag_name,
                    &field_name,
                ));

                match result {
                    Ok(()) => {
                        log::info!(
                            "Dropped fulltext index '{}' on {}.{}",
                            self.index_name,
                            tag_name,
                            field_name
                        );
                    }
                    Err(SearchError::IndexNotFound(_)) => {
                        if self.if_exists {
                            log::warn!(
                                "Fulltext index '{}' does not exist, skipping",
                                self.index_name
                            );
                        } else {
                            return Err(DBError::search(format!(
                                "Index not found: {}",
                                self.index_name
                            )));
                        }
                    }
                    Err(e) => {
                        return Err(DBError::search(e.to_string()));
                    }
                }
            }
            None => {
                return Err(DBError::query(format!(
                    "Invalid fulltext index name format: '{}'",
                    self.index_name
                )));
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
        "DropFulltextIndexExecutor"
    }

    fn description(&self) -> &str {
        "Executor for dropping full-text indexes"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.stats_mut()
    }
}
