//! Selection Executor Module
//!
//! Implements the selection operation (σ) from relational algebra.
//! Responsible for filtering rows based on conditions.

pub mod filter;

// Re-export filter executor
pub use filter::FilterExecutor;
