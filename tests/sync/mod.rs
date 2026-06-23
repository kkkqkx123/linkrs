//! Sync Module Integration Tests
//!
//! Sub-modules are conditionally compiled based on feature flags.

mod circuit_breaker;
mod types;

#[cfg(feature = "fulltext-search")]
mod batch_processor;
#[cfg(feature = "fulltext-search")]
mod comprehensive;
#[cfg(feature = "fulltext-search")]
mod dlq_recovery;
#[cfg(feature = "fulltext-search")]
mod edge;
#[cfg(feature = "fulltext-search")]
mod fault_tolerance;
#[cfg(feature = "fulltext-search")]
mod integration;
#[cfg(feature = "fulltext-search")]
mod recovery_e2e;
#[cfg(feature = "fulltext-search")]
mod transaction_basic;
#[cfg(feature = "fulltext-search")]
mod two_pc_protocol;

#[cfg(feature = "qdrant")]
mod vector_sync;
#[cfg(feature = "qdrant")]
mod vector_transaction;
