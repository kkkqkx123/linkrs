//! AST module
//!
//! This module provides an AST (Abstract Syntax Tree) design based on enumerations, which reduces the amount of样板代码 and the runtime overhead.

// Definition of basic types
pub mod types;
pub use types::EdgeDirection;
pub use types::OrderDirection as CoreOrderDirection;

// Statement definition
pub mod stmt;
pub use stmt::*;

// Pattern definition
pub mod pattern;

// Full-text search definitions
pub mod fulltext;

// Vector search definitions
pub mod vector;

// Utility functions
pub mod utils;
pub use utils::*;
