//! The `trait` for defining the context in which expressions are evaluated
//!
//! Provide a unified context interface for evaluating expressions in graph databases.
//!
//! Note: This trait is used for the evaluation of runtime expressions.
//! For compilation-time analysis, please use `ExpressionAnalysisContext`.

use crate::core::Value;
use crate::query::executor::expression::functions::OwnedFunctionRef;

/// The “expression evaluation context trait”
///
/// Provide a unified context interface for evaluating graph database expressions.
///
/// Note: This trait is used for the evaluation of runtime expressions.
/// For compilation-time analysis, please use `ExpressionAnalysisContext`.
pub trait ExpressionContext {
    /// Obtain the value of the variable
    fn get_variable(&self, name: &str) -> Option<Value>;

    /// Setting variable values
    fn set_variable(&mut self, name: String, value: Value);

    /// Obtain a function reference
    fn get_function(&self, name: &str) -> Option<OwnedFunctionRef> {
        let _ = name;
        None
    }

    /// Check whether the context supports caching.
    fn supports_cache(&self) -> bool {
        false
    }

    /// Obtain the cache manager (if available).
    ///
    /// The caching function has been removed; the result is “None”.
    fn get_cache(&mut self) -> Option<&mut ()> {
        None
    }
}
