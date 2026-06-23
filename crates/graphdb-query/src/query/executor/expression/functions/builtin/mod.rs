//! Module for implementing built-in functions
//!
//! Provide the specific implementations of all built-in functions, organized by function category.
//!
//! Function registration is now performed directly through FunctionRegistry::register_all_builtin_functions.
//! Using a static distribution mechanism, the function is directly called via the BuiltinFunction enumeration.

// The macro module must be loaded first so that it can be used by other modules.
#[macro_use]
pub mod macros;

pub mod aggregate;
pub mod container;
pub mod conversion;
pub mod datetime;
pub mod geography;
pub mod graph;
pub mod math;
pub mod path;
pub mod regex;
pub mod string;
pub mod utility;
pub mod vector;
