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

mod common;
#[path = "optimizer/cost.rs"]
pub mod cost;
#[path = "optimizer/cost_based.rs"]
pub mod cost_based;
#[path = "optimizer/cost_based_strategies.rs"]
pub mod cost_based_strategies;
#[path = "optimizer/heuristic.rs"]
pub mod heuristic;
#[path = "optimizer/heuristic_coverage.rs"]
pub mod heuristic_coverage;
#[path = "optimizer/result_equivalence.rs"]
pub mod result_equivalence;
