//! Implementation of the default expression evaluation context
//!
//! Provide context management during the evaluation of expressions.
//!
//! Note: This context is used for the evaluation of runtime expressions.
//! For compilation-time analysis, please use `ExpressionAnalysisContext`.

use crate::core::Value;
use crate::query::executor::expression::evaluation_context::graph_storage::GraphStorageRef;
use crate::query::executor::expression::functions::global_registry_ref;
use crate::storage::StorageReader;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// The evaluation context of the default expression
///
/// Provide the contextual environment required for evaluating the expression, including:
/// Variable storage
/// Function registration (using a global function registry)
/// Optional graph storage accessor for graph algorithm functions
///
/// Note: This context is used for the evaluation of runtime expressions.
/// For compilation-time analysis, please use `ExpressionAnalysisContext`.
#[derive(Debug)]
pub struct DefaultExpressionContext {
    /// Variable storage
    variables: HashMap<String, Value>,
    /// Query parameter name → value map (resolves `Expression::Parameter`).
    parameters: Option<Arc<HashMap<String, Value>>>,
    /// Session variable name → value snapshot (resolves
    /// `Expression::SessionVariable`).
    session_variables: Option<Arc<HashMap<String, Value>>>,
    /// Optional graph storage for graph algorithm functions
    storage: Option<Arc<RwLock<dyn StorageReader>>>,
    /// Space name for graph storage access
    space: String,
}

impl DefaultExpressionContext {
    /// Create a new context without graph storage access.
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            parameters: None,
            session_variables: None,
            storage: None,
            space: String::new(),
        }
    }

    /// Create a new context with graph storage access.
    pub fn with_storage(storage: Arc<RwLock<dyn StorageReader>>, space: String) -> Self {
        Self {
            variables: HashMap::new(),
            parameters: None,
            session_variables: None,
            storage: Some(storage),
            space,
        }
    }

    /// Attach a parameter name → value map so `Expression::Parameter`
    /// references resolve at evaluation time.
    pub fn with_parameters(mut self, parameters: Arc<HashMap<String, Value>>) -> Self {
        self.parameters = Some(parameters);
        self
    }

    /// Attach a session variable snapshot so `Expression::SessionVariable`
    /// references resolve at evaluation time.
    pub fn with_session_variables(mut self, variables: Arc<HashMap<String, Value>>) -> Self {
        self.session_variables = Some(variables);
        self
    }

    /// Add a variable
    pub fn add_variable(mut self, name: String, value: Value) -> Self {
        self.variables.insert(name, value);
        self
    }

    /// Add variables in batches
    pub fn with_variables<I>(mut self, variables: I) -> Self
    where
        I: IntoIterator<Item = (String, Value)>,
    {
        for (name, value) in variables {
            self.variables.insert(name, value);
        }
        self
    }

    /// Create a DefaultExpressionContext from the ExecutionContext.
    ///
    /// Copy all variables from the ExecutionContext to the new DefaultExpressionContext.
    pub fn from_execution_context(ctx: &crate::query::executor::base::ExecutionContext) -> Self {
        Self {
            variables: ctx.variables.read().clone(),
            parameters: None,
            session_variables: None,
            storage: None,
            space: String::new(),
        }
    }

    /// Synchronize the variable back to the ExecutionContext.
    ///
    /// Synchronize all variables from the current DefaultExpressionContext to the ExecutionContext.
    pub fn sync_to_execution_context(self, ctx: &crate::query::executor::base::ExecutionContext) {
        for (name, value) in self.variables {
            ctx.set_variable(name, value);
        }
    }

    /// Get all variables for debugging
    pub fn get_all_variables(&self) -> &HashMap<String, Value> {
        &self.variables
    }
}

impl Default for DefaultExpressionContext {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::query::executor::expression::evaluator::traits::ExpressionContext
    for DefaultExpressionContext
{
    fn get_variable(&self, name: &str) -> Option<Value> {
        self.variables.get(name).cloned()
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }

    fn get_parameter(&self, name: &str) -> Option<Value> {
        self.parameters
            .as_ref()
            .and_then(|params| params.get(name).cloned())
    }

    fn get_session_variable(
        &self,
        name: &str,
    ) -> Result<Value, crate::query::executor::expression::ExpressionError> {
        self.session_variables
            .as_ref()
            .and_then(|vars| vars.get(name).cloned())
            .ok_or_else(|| {
                crate::query::executor::expression::ExpressionError::type_error(format!(
                    "Session variable `{}` is not defined",
                    name
                ))
            })
    }

    fn get_function(
        &self,
        name: &str,
    ) -> Option<crate::query::executor::expression::functions::OwnedFunctionRef> {
        let registry = global_registry_ref();
        registry
            .get_builtin(name)
            .map(|f| {
                crate::query::executor::expression::functions::OwnedFunctionRef::Builtin(f.clone())
            })
            .or_else(|| {
                registry.get_custom(name).map(|f| {
                    crate::query::executor::expression::functions::OwnedFunctionRef::Custom(
                        f.clone(),
                    )
                })
            })
    }

    fn get_graph_storage(&self) -> Option<GraphStorageRef> {
        self.storage
            .as_ref()
            .map(|s| GraphStorageRef::new(s.clone(), self.space.clone()))
    }
}
