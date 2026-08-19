//! AST module
//!
//! This module provides an AST (Abstract Syntax Tree) design based on enumerations, which reduces the amount of boilerplate code and the runtime overhead.

// Definition of basic types
pub mod types;
pub use types::EdgeDirection;
pub use types::OrderDirection as CoreOrderDirection;

// Statement definition
pub mod stmt;
pub use stmt::*;

// Statement helper macros (loaded first so other modules can use them).
#[macro_use]
pub mod macros;

// Pattern definition
pub mod pattern;

// Full-text search definitions
pub mod fulltext;

// Vector search definitions
pub mod vector;

// Utility functions
pub mod utils;
pub use utils::*;
