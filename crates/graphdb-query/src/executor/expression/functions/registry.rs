//! Function Registry
//!
//! Provide functions for registration, lookup, and execution.
//! The specific implementation of the function is located in the builtin submodule.

mod builtin;
mod custom;
mod table;
mod udf;

#[cfg(test)]
mod tests;

pub use udf::DynamicUdfInfo;

use super::{BuiltinFunction, CustomFunction, TableFunction};
use crate::executor::expression::evaluation_context::graph_storage::GraphStorageRef;
use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use graphdb_core::DataType;
use graphdb_core::Value;
use std::collections::HashMap;
use std::sync::Arc;

use udf::LoadedExtension;

/// Registry entry carrying the function and its static return type.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub function: BuiltinFunction,
    pub return_type: DataType,
}

/// Function Registry
///
/// Using a static distribution mechanism, functions are called directly through the BuiltinFunction and CustomFunction enumerations.
/// The overhead associated with dynamic distribution (dyn) was avoided.
#[derive(Debug)]
pub struct FunctionRegistry {
    /// Built-in function mapping (function name -> RegistryEntry)
    builtin_functions: HashMap<String, RegistryEntry>,
    /// Custom function mapping (function name -> CustomFunction)
    custom_functions: HashMap<String, CustomFunction>,
    /// Table function mapping (function name -> Box<dyn TableFunction>)
    table_functions: HashMap<String, Box<dyn TableFunction>>,
    /// Dynamically loaded UDF libraries (upper-cased name -> extension metadata)
    dynamic_libraries: HashMap<String, LoadedExtension>,
}

impl Default for FunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            builtin_functions: HashMap::new(),
            custom_functions: HashMap::new(),
            table_functions: HashMap::new(),
            dynamic_libraries: HashMap::new(),
        };
        registry.register_all_builtin_functions();
        registry.register_builtin_table_functions();
        registry
    }

    fn resolve_entry(&self, name: &str) -> Option<&RegistryEntry> {
        let upper = name.to_uppercase();
        self.builtin_functions.get(&upper)
    }

    /// Check whether the function exists.
    pub fn contains(&self, name: &str) -> bool {
        let upper_name = name.to_uppercase();
        self.builtin_functions.contains_key(&upper_name)
            || self.custom_functions.contains_key(&upper_name)
    }

    /// Obtain all function names
    pub fn function_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.builtin_functions.keys().map(|s| s.as_str()).collect();
        names.extend(self.custom_functions.keys().map(|s| s.as_str()));
        names
    }

    /// Execute a function (based on its name)
    pub fn execute(&self, name: &str, args: &[Value]) -> Result<Value, ExpressionError> {
        let upper_name = name.to_uppercase();
        if let Some(entry) = self.builtin_functions.get(&upper_name) {
            return entry.function.execute(args);
        }
        if let Some(func) = self.custom_functions.get(&upper_name) {
            return func.execute(args);
        }

        Err(ExpressionError::new(
            ExpressionErrorType::UndefinedFunction,
            format!("Undefined function: {}", name),
        ))
    }

    /// Execute a function with graph storage access
    pub fn execute_with_storage(
        &self,
        name: &str,
        args: &[Value],
        storage: &GraphStorageRef,
    ) -> Result<Value, ExpressionError> {
        let upper_name = name.to_uppercase();
        if let Some(entry) = self.builtin_functions.get(&upper_name) {
            return entry.function.execute_with_storage(args, storage);
        }
        if let Some(func) = self.custom_functions.get(&upper_name) {
            return func.execute(args);
        }
        Err(ExpressionError::new(
            ExpressionErrorType::UndefinedFunction,
            format!("Undefined function: {}", name),
        ))
    }
}

/// Global function registry instance
pub fn global_registry() -> Arc<FunctionRegistry> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Arc<FunctionRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Arc::new(FunctionRegistry::new()))
        .clone()
}

/// Obtain a static reference to the global function registry.
///
/// Used in scenarios where it is necessary to retrieve a function reference (such as in ExpressionContext::get_function).
pub fn global_registry_ref() -> &'static FunctionRegistry {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<FunctionRegistry> = OnceLock::new();
    REGISTRY.get_or_init(FunctionRegistry::new)
}
