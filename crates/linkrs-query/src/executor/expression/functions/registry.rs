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
use linkrs_core::DataType;
use linkrs_core::Value;
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use udf::LoadedExtension;

/// Registry entry carrying the function and its static return type.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub function: BuiltinFunction,
    pub return_type: DataType,
}

/// Exportable metadata for a registered builtin function.
#[derive(Debug, Clone)]
pub struct FunctionSpec {
    pub name: String,
    pub arity: usize,
    pub variadic: bool,
    pub description: String,
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

    /// Export metadata for every registered spelling. The map key is the
    /// registered spelling, so compatibility aliases appear alongside their
    /// canonical names instead of collapsing into duplicates.
    pub fn function_specs(&self) -> Vec<FunctionSpec> {
        let mut specs: Vec<FunctionSpec> = self
            .builtin_functions
            .iter()
            .map(|(registered, entry)| {
                let canonical = entry.function.name().to_string();
                let name = if registered == &canonical.to_uppercase() {
                    canonical
                } else {
                    registered.clone()
                };
                FunctionSpec {
                    name,
                    arity: entry.function.arity(),
                    variadic: entry.function.is_variadic(),
                    description: entry.function.description().to_string(),
                }
            })
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs.dedup_by(|a, b| a.name == b.name && a.arity == b.arity);
        specs
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

/// Process-wide function registry.
///
/// Extensions are process resources: loading a dynamic library makes its
/// symbols available to every session. A single registry therefore backs
/// both query execution and extension management, so `SHOW EXTENSIONS`
/// observes exactly what was loaded.
pub fn global_registry() -> Arc<RwLock<FunctionRegistry>> {
    Arc::clone(registry_cell())
}

/// Obtain a static reference to the process-wide function registry.
pub fn global_registry_ref() -> &'static RwLock<FunctionRegistry> {
    registry_cell().as_ref()
}

fn registry_cell() -> &'static Arc<RwLock<FunctionRegistry>> {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<Arc<RwLock<FunctionRegistry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Arc::new(RwLock::new(FunctionRegistry::new())))
}
