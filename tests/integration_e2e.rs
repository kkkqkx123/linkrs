//! E2E Integration Tests for GraphDB
//!
//! This file serves as the entry point for E2E tests.
//! Actual tests are organized in the e2e/ subdirectory.

mod e2e;

// Re-export test modules for direct access
pub use e2e::common;
pub use e2e::extended_types;
pub use e2e::optimizer;
pub use e2e::schema_manager;
pub use e2e::social_network;
