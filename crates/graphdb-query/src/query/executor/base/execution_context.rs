use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use super::execution_result::ExecutionResult;
use super::MemoryBudget;
use crate::core::Value;
use crate::query::executor::expression::functions::global_registry_ref;
use crate::query::executor::expression::functions::OwnedFunctionRef;
use crate::query::executor::streaming::pool::SharedScheduler;
use crate::query::validator::context::ExpressionAnalysisContext;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
#[cfg(feature = "fulltext-search")]
use crate::search::tantivy_index::TantivySearchEngine;
use crate::storage::StorageClient;
#[cfg(feature = "qdrant")]
use crate::sync::VectorSyncCoordinator;

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub results: Arc<RwLock<HashMap<String, ExecutionResult>>>,
    pub variables: Arc<RwLock<HashMap<String, crate::core::Value>>>,
    pub expression_context: Arc<ExpressionAnalysisContext>,
    #[cfg(feature = "fulltext-search")]
    pub search_engine: Option<Arc<TantivySearchEngine>>,
    #[cfg(feature = "fulltext-search")]
    pub fulltext_manager: Option<Arc<FulltextIndexManager>>,
    #[cfg(feature = "qdrant")]
    pub vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    pub storage: Option<Arc<RwLock<dyn StorageClient>>>,
    pub space_name: Option<String>,
    pub parameters: Arc<HashMap<String, crate::core::Value>>,
    /// Per-query memory budget for blocking operators.
    pub memory_budget: MemoryBudget,
    /// Maximum intra-query workers (P8). 1 = serial only.
    pub max_workers: usize,
    /// Server-assigned query ID for KILL QUERY / cancellation support.
    pub query_id: u64,
    /// Streaming chunk size (rows per chunk).
    pub chunk_size: usize,
    /// Maximum buffered chunks before back-pressure.
    pub max_buffered_chunks: usize,
    /// M6: Engine-level shared scheduler.
    pub shared_scheduler: Option<Arc<SharedScheduler>>,
}

impl ExecutionContext {
    pub const DEFAULT_CHUNK_SIZE: usize = 1024;
    pub const DEFAULT_MAX_BUFFERED_CHUNKS: usize = 10;

    pub fn new(expression_context: Arc<ExpressionAnalysisContext>) -> Self {
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
            variables: Arc::new(RwLock::new(HashMap::new())),
            expression_context,
            #[cfg(feature = "fulltext-search")]
            search_engine: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            storage: None,
            space_name: None,
            parameters: Arc::new(HashMap::new()),
            memory_budget: MemoryBudget::default_budget(),
            max_workers: 1,
            query_id: 0,
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
            max_buffered_chunks: Self::DEFAULT_MAX_BUFFERED_CHUNKS,
            shared_scheduler: None,
        }
    }

    pub fn with_parameters(
        expression_context: Arc<ExpressionAnalysisContext>,
        parameters: HashMap<String, crate::core::Value>,
    ) -> Self {
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
            variables: Arc::new(RwLock::new(HashMap::new())),
            expression_context,
            #[cfg(feature = "fulltext-search")]
            search_engine: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            storage: None,
            space_name: None,
            parameters: Arc::new(parameters),
            memory_budget: MemoryBudget::default_budget(),
            max_workers: 1,
            query_id: 0,
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
            max_buffered_chunks: Self::DEFAULT_MAX_BUFFERED_CHUNKS,
            shared_scheduler: None,
        }
    }

    #[cfg(feature = "fulltext-search")]
    pub fn with_search_engine(
        expression_context: Arc<ExpressionAnalysisContext>,
        search_engine: Arc<TantivySearchEngine>,
    ) -> Self {
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
            variables: Arc::new(RwLock::new(HashMap::new())),
            expression_context,
            search_engine: Some(search_engine),
            fulltext_manager: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            storage: None,
            space_name: None,
            parameters: Arc::new(HashMap::new()),
            memory_budget: MemoryBudget::default_budget(),
            max_workers: 1,
            query_id: 0,
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
            max_buffered_chunks: Self::DEFAULT_MAX_BUFFERED_CHUNKS,
            shared_scheduler: None,
        }
    }

    /// Create a context with memory limit configured from a config value.
    ///
    /// When `max_memory_per_query` is non-zero it overrides the default
    /// 512 MiB budget; otherwise the default is used.
    pub fn with_memory_limit(
        expression_context: Arc<ExpressionAnalysisContext>,
        max_memory_per_query: u64,
    ) -> Self {
        let budget = if max_memory_per_query > 0 {
            MemoryBudget::new(max_memory_per_query as usize)
        } else {
            MemoryBudget::default_budget()
        };
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
            variables: Arc::new(RwLock::new(HashMap::new())),
            expression_context,
            #[cfg(feature = "fulltext-search")]
            search_engine: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            storage: None,
            space_name: None,
            parameters: Arc::new(HashMap::new()),
            memory_budget: budget,
            max_workers: 1,
            query_id: 0,
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
            max_buffered_chunks: Self::DEFAULT_MAX_BUFFERED_CHUNKS,
            shared_scheduler: None,
        }
    }

    pub fn set_result(&self, name: String, result: ExecutionResult) {
        self.results.write().insert(name, result);
    }

    pub fn get_result(&self, name: &str) -> Option<ExecutionResult> {
        self.results.write().get(name).cloned()
    }

    pub fn set_variable(&self, name: String, value: crate::core::Value) {
        self.variables.write().insert(name, value);
    }

    pub fn get_variable(&self, name: &str) -> Option<crate::core::Value> {
        self.variables.write().get(name).cloned()
    }

    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expression_context
    }

    #[cfg(feature = "fulltext-search")]
    pub fn search_engine(&self) -> Option<&Arc<TantivySearchEngine>> {
        self.search_engine.as_ref()
    }

    pub fn get_param(&self, name: &str) -> Option<&crate::core::Value> {
        self.parameters.get(name)
    }

    pub fn current_space_id(&self) -> Option<u64> {
        self.variables
            .write()
            .get("space_id")
            .and_then(|v| match v {
                Value::Int(id) => Some(*id as u64),
                _ => None,
            })
    }

    pub fn set_space_id(&self, space_id: u64) {
        self.variables
            .write()
            .insert("space_id".to_string(), Value::Int(space_id as i32));
    }
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
            variables: Arc::new(RwLock::new(HashMap::new())),
            expression_context: Arc::new(ExpressionAnalysisContext::new()),
            #[cfg(feature = "fulltext-search")]
            search_engine: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            storage: None,
            space_name: None,
            parameters: Arc::new(HashMap::new()),
            memory_budget: MemoryBudget::default_budget(),
            max_workers: 1,
            query_id: 0,
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
            max_buffered_chunks: Self::DEFAULT_MAX_BUFFERED_CHUNKS,
            shared_scheduler: None,
        }
    }
}

impl crate::query::executor::expression::evaluator::traits::ExpressionContext for ExecutionContext {
    fn get_variable(&self, name: &str) -> Option<Value> {
        self.variables.write().get(name).cloned()
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.write().insert(name, value);
    }

    fn get_function(&self, name: &str) -> Option<OwnedFunctionRef> {
        let registry = global_registry_ref();
        registry
            .get_builtin(name)
            .map(|f| OwnedFunctionRef::Builtin(f.clone()))
            .or_else(|| {
                registry
                    .get_custom(name)
                    .map(|f| OwnedFunctionRef::Custom(f.clone()))
            })
    }
}
