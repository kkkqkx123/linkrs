//! Plan node macro definition
//!
//! Provide macros to simplify the definition of plan nodes and reduce boilerplate code.
//!
//! # Refactoring changes
//! Remove the dependency on `ast::Variable` and use `String` instead.

mod enum_methods;
mod single_input;
mod binary_input;
mod dependency;
mod data_operation;
