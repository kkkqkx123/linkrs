//! Pipeline Internal Processing Tests
//!
//! Test coverage:
//! - Parsing: Lexer edge cases, parser errors, AST validation
//! - Validation: Semantic checks, type checks, expression analysis
//! - Planning: Plan construction, transformation, caching
//!
//! These tests focus on internal processing correctness, complementing
//! the end-to-end tests in dcl/ddl/dml/dql directories.

pub mod parsing;
pub mod planning;
pub mod validation;
