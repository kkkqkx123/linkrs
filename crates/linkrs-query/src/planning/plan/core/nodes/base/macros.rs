//! Plan node macro definition
//!
//! Provide macros to simplify the definition of plan nodes and reduce boilerplate code.
//!
//! # Refactoring changes
//! Remove the dependency on `ast::Variable` and use `String` instead.

mod binary_input;
mod data_operation;
mod dependency;
mod enum_methods;
mod single_input;
