//! ValueRowContext: Expression evaluation context for streaming rows
//!
//! Provides expression evaluation context for Vec<Value> rows.
//! Runtime uses SlotId-based access via SlotLayout; column-name lookup
//! at runtime is prohibited — all Variable references are resolved through
//! the layout.

use crate::core::Value;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::streaming::slot::{SlotId, SlotLayout};
use std::collections::HashMap;
use std::sync::Arc;

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
}

impl ValueRowContext {
    /// Create a new context from a row and slot layout.
    pub fn new(row: Vec<Value>, layout: Arc<SlotLayout>) -> Self {
        Self {
            row,
            variables: HashMap::new(),
            layout,
        }
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
        // Column-name lookup by hash map is intentionally removed; all
        // Variable(name) expressions must be resolvable through the layout.
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
}

impl<'a> BorrowedRowContext<'a> {
    pub fn new(row: &'a [Value], layout: Arc<SlotLayout>) -> Self {
        Self {
            row,
            variables: HashMap::new(),
            layout,
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let row = vec![
            Value::Int(1),
            Value::String("test".to_string()),
            Value::Bool(true),
        ];
        let col_names = vec!["id".to_string(), "name".to_string(), "active".to_string()];
        let layout = Arc::new(SlotLayout::from_names(&col_names));

        let context = ValueRowContext::new(row, layout);

        assert_eq!(context.get_variable("id"), Some(Value::Int(1)));
        assert_eq!(
            context.get_variable("name"),
            Some(Value::String("test".to_string()))
        );
        assert_eq!(context.get_variable("active"), Some(Value::Bool(true)));
    }

    #[test]
    fn test_variable_storage() {
        let row = vec![Value::Int(1)];
        let col_names = vec!["id".to_string()];
        let layout = Arc::new(SlotLayout::from_names(&col_names));
        let mut context = ValueRowContext::new(row, layout);

        context.set_variable("var1".to_string(), Value::String("hello".to_string()));
        context.set_variable("var2".to_string(), Value::Int(42));

        assert_eq!(
            context.get_variable("var1"),
            Some(Value::String("hello".to_string()))
        );
        assert_eq!(context.get_variable("var2"), Some(Value::Int(42)));
    }

    #[test]
    fn test_missing_column() {
        let row = vec![Value::Int(1), Value::String("test".to_string())];
        let col_names = vec!["id".to_string(), "name".to_string()];
        let layout = Arc::new(SlotLayout::from_names(&col_names));

        let context = ValueRowContext::new(row, layout);

        assert_eq!(context.get_variable("nonexistent"), None);
        assert_eq!(context.get_variable("var_not_set"), None);
    }
}
