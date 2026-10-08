use super::super::TableFunction;
use super::FunctionRegistry;
use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use linkrs_core::Value;

impl FunctionRegistry {
    /// Register a table function
    pub fn register_table_function(&mut self, func: Box<dyn TableFunction>) {
        let upper_name = func.name().to_uppercase();
        self.table_functions.insert(upper_name, func);
    }

    /// Check if a table function exists
    pub fn contains_table_function(&self, name: &str) -> bool {
        self.table_functions.contains_key(&name.to_uppercase())
    }

    /// Execute a table function by name
    pub fn execute_table_function(
        &self,
        name: &str,
        args: &[Value],
    ) -> Result<Vec<Vec<Value>>, ExpressionError> {
        let upper_name = name.to_uppercase();
        if let Some(func) = self.table_functions.get(&upper_name) {
            return func.execute(args);
        }
        Err(ExpressionError::new(
            ExpressionErrorType::UndefinedFunction,
            format!("Undefined table function: {}", name),
        ))
    }

    /// Get all table function names
    pub fn table_function_names(&self) -> Vec<&str> {
        self.table_functions.keys().map(|s| s.as_str()).collect()
    }

    /// Register built-in table functions
    pub(super) fn register_builtin_table_functions(&mut self) {
        use super::super::BuiltinTableFunction;
        self.register_table_function(Box::new(BuiltinTableFunction::ReadCsv));
    }
}
