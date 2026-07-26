//! Plan Validation Module
//!
//! Provides validation utilities for execution plans, including:
//! - Cycle detection: Ensures plan graphs are acyclic
//! - Schema validation: Checks schema compatibility between nodes

pub mod cycle_detection;
pub mod schema_validation;

pub use cycle_detection::CycleDetector;
pub use schema_validation::SchemaValidator;
