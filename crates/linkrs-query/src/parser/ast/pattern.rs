//! Pattern AST re-export shim.
//!
//! The pattern data structures now live in `linkrs-core::types::pattern` so
//! that `SubqueryBody` can carry already-parsed patterns. Everything re-exports
//! transparently; import paths inside the query crate are unchanged.

pub use linkrs_core::types::pattern::*;
