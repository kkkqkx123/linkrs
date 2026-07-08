use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use super::execution_result::ExecutionResult;
use crate::core::Value;
use crate::query::executor::expression::functions::global_registry_ref;
use crate::query::executor::expression::functions::OwnedFunctionRef;
use crate::query::validator::context::ExpressionAnalysisContext;
#[cfg(feature = "fulltext-search")]
use crate::search::tantivy_index::TantivySearchEngine;

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub results: Arc<RwLock<HashMap<String, ExecutionResult>>>,
    pub variables: Arc<RwLock<HashMap<String, crate::core::Value>>>,
    pub expression_context: Arc<ExpressionAnalysisContext>,
    #[cfg(feature = "fulltext-search")]
    pub search_engine: Option<Arc<TantivySearchEngine>>,
    pub parameters: Arc<HashMap<String, crate::core::Value>>,
}

impl ExecutionContext {
    pub fn new(expression_context: Arc<ExpressionAnalysisContext>) -> Self {
        Self {
            results: Arc::new(RwLock::new(HashMap::new())),
            variables: Arc::new(RwLock::new(HashMap::new())),
            expression_context,
            #[cfg(feature = "fulltext-search")]
            search_engine: None,
            parameters: Arc::new(HashMap::new()),
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
            parameters: Arc::new(parameters),
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
            parameters: Arc::new(HashMap::new()),
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
            parameters: Arc::new(HashMap::new()),
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
