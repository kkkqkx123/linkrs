//! Optimizer Batch Module
//!
//! This module defines optimization batches that apply rules in phases with
//! proper fixed-point iteration, fingerprinting, and diagnostics.
//!
//! # Design
//!
//! Each batch:
//! - Has a specific optimization goal (normalize, pushdown, pruning, etc.)
//! - Uses whole-plan fixed-point iteration
//! - Tracks explicit fingerprints before/after
//! - Has iteration limits and stop reasons
//! - Records rule hits and diagnostics
//!
//! The module was split from the original single `batch.rs` into focused
//! submodules:
//! - `subquery_host`: the [`SubqueryHostFinder`] visitor and the
//!   `subtree_contains_expression_subqueries` gate
//! - `rewriter`: the per-batch [`BatchNodeRewriter`]
//! - `types`: batch enums / statistics / result types
//! - `optimizer`: the [`BatchOptimizer`] driver

mod optimizer;
mod rewriter;
mod subquery_host;
mod types;

pub use optimizer::BatchOptimizer;
pub use types::{BatchStatistics, BatchStopReason, OptimizationBatch, OptimizationResult};
