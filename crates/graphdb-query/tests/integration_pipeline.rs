//! Pipeline Internal Processing Integration Tests
//!
//! This file serves as the entry point for pipeline tests.
//! Actual tests are organized in the pipeline/ subdirectory.
//!
//! Test coverage:
//! - Parsing: Lexer edge cases, parser errors, AST validation
//! - Validation: Semantic checks, type checks, expression analysis
//! - Planning: Plan construction, transformation, caching

mod common;
mod pipeline;
