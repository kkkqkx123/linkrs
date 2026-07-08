//! Implementation in the context of row expressions
//!
//! Provide dedicated context implementations for the Join operation and the evaluation of row-level expressions.
//! Support accessing row data by column name and column index.

use crate::core::Value;
use std::collections::HashMap;

/// Context of the expression line
///
/// A context implementation specifically designed for evaluating expressions on row data
/// Two access modes are supported:
/// Access by column name: via the col_name_index mapping
/// 2. Access by variable name: via the variables mapping
#[derive(Debug, Clone)]
pub struct RowExpressionContext {
    /// Data from the current row
    row: Vec<Value>,
    /// Column name index mapping (for quick searching)
    col_name_index: HashMap<String, usize>,
    /// Extra variables (used to store intermediate results of calculations)
    variables: HashMap<String, Value>,
}

impl RowExpressionContext {
    /// Create a new line context.
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

    /// Create a context based on the existing data.
    pub fn from_dataset(row: &[Value], col_names: &[String]) -> Self {
        Self::new(row.to_vec(), col_names.to_vec())
    }

    /// Retrieve values based on column names.
    pub fn get_value_by_name(&self, name: &str) -> Option<&Value> {
        let result = self
            .col_name_index
            .get(name)
            .and_then(|&idx| self.row.get(idx));
        result
    }
}

impl crate::query::executor::expression::evaluator::traits::ExpressionContext
    for RowExpressionContext
{
    fn get_variable(&self, name: &str) -> Option<Value> {
        // First, check the variable mapping.
        if let Some(value) = self.variables.get(name) {
            return Some(value.clone());
        }

        // Then check the column names (it is possible to access the column names as variables).
        if let Some(value) = self.get_value_by_name(name) {
            return Some(value.clone());
        }

        None
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }

    fn get_function(
        &self,
        name: &str,
    ) -> Option<crate::query::executor::expression::functions::OwnedFunctionRef> {
        let _ = name;
        None
    }
}
