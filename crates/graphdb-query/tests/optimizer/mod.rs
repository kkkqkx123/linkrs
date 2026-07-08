//! Optimizer Module Tests
//!
//! Test coverage:
//! - Heuristic optimization rules
//! - Cost-based optimization strategies
//! - Cost estimation and selectivity
//! - Statistics management
//!
//! These tests focus on optimizer internal correctness, complementing
//! the end-to-end optimizer tests in tests/dql/optimizer.rs

pub mod cost;
pub mod cost_based;
pub mod cost_based_strategies;
pub mod heuristic;
pub mod heuristic_coverage;
pub mod result_equivalence;
