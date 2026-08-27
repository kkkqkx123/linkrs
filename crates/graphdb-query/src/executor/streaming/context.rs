//! ValueRowContext: Expression evaluation context for streaming rows
//!
//! Provides expression evaluation context for Vec<Value> rows.
//! Runtime uses SlotId-based access via SlotLayout; column-name lookup
//! at runtime is prohibited — all Variable references are resolved through
//! the layout.

use crate::core::Value;
use crate::executor::expression::evaluator::traits::ExpressionContext;
use crate::executor::expression::ExpressionError;
use crate::executor::streaming::slot::{SlotId, SlotLayout};
use crate::executor::streaming::subquery::{EvalEnv, SubqueryExecutor};
use std::collections::HashMap;
use std::sync::Arc;

use super::parameters::{ParameterFrame, ParameterSchema};

/// Row context for expression evaluation with Value types
///
/// Every context carries an [`Arc<SlotLayout>`] so that `get_variable(name)`
/// resolves through `layout.slot_id(name)` → `row[slot]` without any
/// separate name-to-index map or runtime fallback.
pub struct ValueRowContext {
    /// Column values (as Values, no conversion needed)
    row: Vec<Value>,
    /// Extra variables for expression evaluation
    variables: HashMap<String, Value>,
    /// Slot layout — always set; drives all variable resolution
    layout: Arc<SlotLayout>,
    /// Parameter name→value map (shared via Arc across rows).
    parameters: Option<Arc<HashMap<String, Value>>>,
    /// Session variable name→value snapshot (shared via Arc across rows).
    session_variables: Option<Arc<HashMap<String, Value>>>,
    /// Expression-level subquery executor of the hosting operator.
    subquery_executor: Option<Arc<SubqueryExecutor>>,
}

impl ValueRowContext {
    /// Create a new context from a row and slot layout.
    pub fn new(row: Vec<Value>, layout: Arc<SlotLayout>) -> Self {
        Self {
            row,
            variables: HashMap::new(),
            layout,
            parameters: None,
            session_variables: None,
            subquery_executor: None,
        }
    }

    /// Create a new context with parameter values for `@name` resolution.
    pub fn with_parameters(
        row: Vec<Value>,
        layout: Arc<SlotLayout>,
        parameters: Arc<HashMap<String, Value>>,
    ) -> Self {
        Self {
            row,
            variables: HashMap::new(),
            layout,
            parameters: Some(parameters),
            session_variables: None,
            subquery_executor: None,
        }
    }

    /// Create a new context from a full evaluation environment.
    pub fn with_env(row: Vec<Value>, layout: Arc<SlotLayout>, env: &EvalEnv) -> Self {
        Self {
            row,
            variables: HashMap::new(),
            layout,
            parameters: env.params.clone(),
            session_variables: env.session_variables.clone(),
            subquery_executor: env.subquery_executor.clone(),
        }
    }

    /// Build a name→value map from schema and frame.
    pub fn build_parameter_map(
        schema: &ParameterSchema,
        frame: &ParameterFrame,
    ) -> HashMap<String, Value> {
        schema
            .params
            .iter()
            .filter_map(|p| frame.get(p.slot).map(|v| (p.name.clone(), v.clone())))
            .collect()
    }

    /// Create a new context by building a layout from column names.
    ///
    /// This is a convenience for sites that only have column-name strings
    /// (e.g. legacy signatures or helper functions).  The layout is
    /// created once and reused for all subsequent `get_variable()` calls
    /// so there is never a name-map fallback at runtime.
    pub fn from_names(row: Vec<Value>, col_names: Vec<String>) -> Self {
        let layout = Arc::new(SlotLayout::from_names(&col_names));
        Self::new(row, layout)
    }
}

impl ExpressionContext for ValueRowContext {
    fn get_variable(&self, name: &str) -> Option<Value> {
        // First check explicit variables (e.g. bindings from Apply)
        if let Some(value) = self.variables.get(name) {
            return Some(value.clone());
        }
        // Slot-based access via layout — the ONLY resolution path at runtime.
        self.layout
            .slot_id(name)
            .and_then(|slot_id| self.row.get(slot_id).cloned())
    }

    fn get_variable_by_slot(&self, slot: SlotId) -> Option<Value> {
        self.row.get(slot).cloned()
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }

    fn get_parameter(&self, name: &str) -> Option<Value> {
        self.parameters.as_ref().and_then(|p| p.get(name).cloned())
    }

    fn get_session_variable(&self, name: &str) -> Result<Value, ExpressionError> {
        self.session_variables
            .as_ref()
            .and_then(|vars| vars.get(name).cloned())
            .ok_or_else(|| {
                ExpressionError::type_error(format!("Session variable `{}` is not defined", name))
            })
    }

    fn execute_exists(
        &mut self,
        body: &crate::core::types::expr::SubqueryBody,
    ) -> Result<bool, ExpressionError> {
        self.subquery_executor
            .as_ref()
            .ok_or_else(|| {
                ExpressionError::type_error("Subquery execution not supported in this context")
            })?
            .execute_exists(body, self.layout.clone(), self.row.clone())
    }

    fn contains_subquery(
        &mut self,
        body: &crate::core::types::expr::SubqueryBody,
        value: &Value,
    ) -> Result<Value, ExpressionError> {
        if value.is_null() {
            return Ok(Value::Bool(false));
        }
        let found = self
            .subquery_executor
            .as_ref()
            .ok_or_else(|| {
                ExpressionError::type_error("Subquery execution not supported in this context")
            })?
            .execute_contains(body, self.layout.clone(), self.row.clone(), value)?;
        Ok(Value::Bool(found))
    }
}

/// A light-weight expression context that borrows the row data instead
/// of cloning it.  Use this in hot paths (e.g. Filter) where the
/// expression only reads from the row and does not take ownership.
///
/// Individual values returned by `get_variable` are still cloned (that
/// is inherent in the `ExpressionContext` trait), but the `Vec<Value>`
/// wrapper itself is not — saving one allocation per row.
pub struct BorrowedRowContext<'a> {
    row: &'a [Value],
    variables: HashMap<String, Value>,
    layout: Arc<SlotLayout>,
    parameters: Option<Arc<HashMap<String, Value>>>,
    /// Session variable name→value snapshot (shared via Arc across rows).
    session_variables: Option<Arc<HashMap<String, Value>>>,
    /// Expression-level subquery executor of the hosting operator.
    subquery_executor: Option<Arc<SubqueryExecutor>>,
}

impl<'a> BorrowedRowContext<'a> {
    pub fn new(row: &'a [Value], layout: Arc<SlotLayout>) -> Self {
        Self {
            row,
            variables: HashMap::new(),
            layout,
            parameters: None,
            session_variables: None,
            subquery_executor: None,
        }
    }

    /// Create with parameter values for `@name` resolution.
    pub fn with_parameters(
        row: &'a [Value],
        layout: Arc<SlotLayout>,
        parameters: Arc<HashMap<String, Value>>,
    ) -> Self {
        Self {
            row,
            variables: HashMap::new(),
            layout,
            parameters: Some(parameters),
            session_variables: None,
            subquery_executor: None,
        }
    }

    /// Create from a full evaluation environment (parameters + subquery
    /// executor).
    pub fn with_env(row: &'a [Value], layout: Arc<SlotLayout>, env: &EvalEnv) -> Self {
        Self {
            row,
            variables: HashMap::new(),
            layout,
            parameters: env.params.clone(),
            session_variables: env.session_variables.clone(),
            subquery_executor: env.subquery_executor.clone(),
        }
    }

    /// Update the row reference and clear per-row variables.
    ///
    /// Call this in a hot loop to reuse the same context across many rows,
    /// avoiding repeated `Arc::clone` on the slot layout.
    pub fn set_row(&mut self, row: &'a [Value]) {
        self.row = row;
        self.variables.clear();
    }
}

impl ExpressionContext for BorrowedRowContext<'_> {
    fn get_variable(&self, name: &str) -> Option<Value> {
        if let Some(value) = self.variables.get(name) {
            return Some(value.clone());
        }
        self.layout
            .slot_id(name)
            .and_then(|slot_id| self.row.get(slot_id).cloned())
    }

    fn get_variable_by_slot(&self, slot: SlotId) -> Option<Value> {
        self.row.get(slot).cloned()
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }

    fn get_parameter(&self, name: &str) -> Option<Value> {
        self.parameters.as_ref().and_then(|p| p.get(name).cloned())
    }

    fn get_session_variable(&self, name: &str) -> Result<Value, ExpressionError> {
        self.session_variables
            .as_ref()
            .and_then(|vars| vars.get(name).cloned())
            .ok_or_else(|| {
                ExpressionError::type_error(format!("Session variable `{}` is not defined", name))
            })
    }

    fn execute_exists(
        &mut self,
        body: &crate::core::types::expr::SubqueryBody,
    ) -> Result<bool, ExpressionError> {
        self.subquery_executor
            .as_ref()
            .ok_or_else(|| {
                ExpressionError::type_error("Subquery execution not supported in this context")
            })?
            .execute_exists(body, self.layout.clone(), self.row.to_vec())
    }

    fn contains_subquery(
        &mut self,
        body: &crate::core::types::expr::SubqueryBody,
        value: &Value,
    ) -> Result<Value, ExpressionError> {
        if value.is_null() {
            return Ok(Value::Bool(false));
        }
        let found = self
            .subquery_executor
            .as_ref()
            .ok_or_else(|| {
                ExpressionError::type_error("Subquery execution not supported in this context")
            })?
            .execute_contains(body, self.layout.clone(), self.row.to_vec(), value)?;
        Ok(Value::Bool(found))
    }
}

/// A context that presents two disjoint row halves as a single combined row
/// without allocating a combined `Vec<Value>`.
///
/// Slots `0..split` map to the left slice; slots `split..` map to the right
/// slice.  Use this in hash join condition evaluation to avoid cloning
/// every column before the condition is checked.
pub struct SplitRowContext<'a> {
    left: &'a [Value],
    right: &'a [Value],
    split: usize,
    variables: HashMap<String, Value>,
    layout: Arc<SlotLayout>,
    parameters: Option<Arc<HashMap<String, Value>>>,
    /// Session variable name→value snapshot (shared via Arc across rows).
    session_variables: Option<Arc<HashMap<String, Value>>>,
    /// Expression-level subquery executor of the hosting operator.
    subquery_executor: Option<Arc<SubqueryExecutor>>,
}

impl<'a> SplitRowContext<'a> {
    pub fn new(left: &'a [Value], right: &'a [Value], layout: Arc<SlotLayout>) -> Self {
        let split = left.len();
        Self {
            left,
            right,
            split,
            variables: HashMap::new(),
            layout,
            parameters: None,
            session_variables: None,
            subquery_executor: None,
        }
    }

    /// Create from a full evaluation environment (parameters + subquery
    /// executor).
    pub fn with_env(
        left: &'a [Value],
        right: &'a [Value],
        layout: Arc<SlotLayout>,
        env: &EvalEnv,
    ) -> Self {
        let split = left.len();
        Self {
            left,
            right,
            split,
            variables: HashMap::new(),
            layout,
            parameters: env.params.clone(),
            session_variables: env.session_variables.clone(),
            subquery_executor: env.subquery_executor.clone(),
        }
    }
}

impl ExpressionContext for SplitRowContext<'_> {
    fn get_variable(&self, name: &str) -> Option<Value> {
        if let Some(value) = self.variables.get(name) {
            return Some(value.clone());
        }
        self.layout
            .slot_id(name)
            .and_then(|slot_id| self.get_variable_by_slot(slot_id))
    }

    fn get_variable_by_slot(&self, slot: SlotId) -> Option<Value> {
        if slot < self.split {
            self.left.get(slot).cloned()
        } else {
            self.right.get(slot - self.split).cloned()
        }
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }

    fn get_parameter(&self, name: &str) -> Option<Value> {
        self.parameters.as_ref().and_then(|p| p.get(name).cloned())
    }

    fn get_session_variable(&self, name: &str) -> Result<Value, ExpressionError> {
        self.session_variables
            .as_ref()
            .and_then(|vars| vars.get(name).cloned())
            .ok_or_else(|| {
                ExpressionError::type_error(format!("Session variable `{}` is not defined", name))
            })
    }

    fn execute_exists(
        &mut self,
        body: &crate::core::types::expr::SubqueryBody,
    ) -> Result<bool, ExpressionError> {
        let mut row = self.left.to_vec();
        row.extend_from_slice(self.right);
        self.subquery_executor
            .as_ref()
            .ok_or_else(|| {
                ExpressionError::type_error("Subquery execution not supported in this context")
            })?
            .execute_exists(body, self.layout.clone(), row)
    }

    fn contains_subquery(
        &mut self,
        body: &crate::core::types::expr::SubqueryBody,
        value: &Value,
    ) -> Result<Value, ExpressionError> {
        if value.is_null() {
            return Ok(Value::Bool(false));
        }
        let mut row = self.left.to_vec();
        row.extend_from_slice(self.right);
        let found = self
            .subquery_executor
            .as_ref()
            .ok_or_else(|| {
                ExpressionError::type_error("Subquery execution not supported in this context")
            })?
            .execute_contains(body, self.layout.clone(), row, value)?;
        Ok(Value::Bool(found))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let row = vec![Value::Int(1), Value::string("test"), Value::Bool(true)];
        let col_names = vec!["id".to_string(), "name".to_string(), "active".to_string()];
        let layout = Arc::new(SlotLayout::from_names(&col_names));

        let context = ValueRowContext::new(row, layout);

        assert_eq!(context.get_variable("id"), Some(Value::Int(1)));
        assert_eq!(context.get_variable("name"), Some(Value::string("test")));
        assert_eq!(context.get_variable("active"), Some(Value::Bool(true)));
    }

    #[test]
    fn test_variable_storage() {
        let row = vec![Value::Int(1)];
        let col_names = vec!["id".to_string()];
        let layout = Arc::new(SlotLayout::from_names(&col_names));
        let mut context = ValueRowContext::new(row, layout);

        context.set_variable("var1".to_string(), Value::string("hello"));
        context.set_variable("var2".to_string(), Value::Int(42));

        assert_eq!(context.get_variable("var1"), Some(Value::string("hello")));
        assert_eq!(context.get_variable("var2"), Some(Value::Int(42)));
    }

    #[test]
    fn test_missing_column() {
        let row = vec![Value::Int(1), Value::string("test")];
        let col_names = vec!["id".to_string(), "name".to_string()];
        let layout = Arc::new(SlotLayout::from_names(&col_names));

        let context = ValueRowContext::new(row, layout);

        assert_eq!(context.get_variable("nonexistent"), None);
        assert_eq!(context.get_variable("var_not_set"), None);
    }
}
