//! ValueRowContext: Expression evaluation context for streaming rows
//!
//! Provides expression evaluation context for Vec<Value> rows.
//! No type conversion needed since data is already in Value format.

use crate::core::Value;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use std::collections::HashMap;

/// Row context for expression evaluation with Value types
///
/// Provides expression evaluation context for Vec<Value> rows.
/// No type conversion needed since data is already in Value format.
pub struct ValueRowContext {
    /// Column values (as Values, no conversion needed)
    row: Vec<Value>,
    /// Column name to index mapping
    col_name_index: HashMap<String, usize>,
    /// Extra variables for expression evaluation
    variables: HashMap<String, Value>,
}

impl ValueRowContext {
    /// Create a new context from a row and column names
    pub fn new(row: Vec<Value>, col_names: Vec<String>) -> Self {
        let col_name_index: HashMap<String, usize> = col_names
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, i))
            .collect();

        Self {
            row,
            col_name_index,
            variables: HashMap::new(),
        }
    }

    /// Get a column value by name
    fn get_value_by_name(&self, name: &str) -> Option<Value> {
        self.col_name_index
            .get(name)
            .and_then(|&idx| self.row.get(idx))
            .cloned()
    }
}

impl ExpressionContext for ValueRowContext {
    fn get_variable(&self, name: &str) -> Option<Value> {
        // First check explicit variables
        if let Some(value) = self.variables.get(name) {
            return Some(value.clone());
        }

        // Then check column names (columns can be accessed as variables)
        self.get_value_by_name(name)
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let row = vec![Value::Int(1), Value::String("test".to_string()), Value::Bool(true)];
        let col_names = vec!["id".to_string(), "name".to_string(), "active".to_string()];

        let context = ValueRowContext::new(row, col_names);

        // Verify column index mappings work
        assert_eq!(context.get_variable("id"), Some(Value::Int(1)));
        assert_eq!(context.get_variable("name"), Some(Value::String("test".to_string())));
        assert_eq!(context.get_variable("active"), Some(Value::Bool(true)));
    }

    #[test]
    fn test_variable_storage() {
        let row = vec![Value::Int(1)];
        let col_names = vec!["id".to_string()];
        let mut context = ValueRowContext::new(row, col_names);

        // Set and retrieve variables
        context.set_variable("var1".to_string(), Value::String("hello".to_string()));
        context.set_variable("var2".to_string(), Value::Int(42));

        assert_eq!(context.get_variable("var1"), Some(Value::String("hello".to_string())));
        assert_eq!(context.get_variable("var2"), Some(Value::Int(42)));
    }

    #[test]
    fn test_missing_column() {
        let row = vec![Value::Int(1), Value::String("test".to_string())];
        let col_names = vec!["id".to_string(), "name".to_string()];

        let context = ValueRowContext::new(row, col_names);

        // Non-existent column should return None
        assert_eq!(context.get_variable("nonexistent"), None);
        // Non-existent variable should return None
        assert_eq!(context.get_variable("var_not_set"), None);
    }
}
