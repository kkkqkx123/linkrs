//! Sync Module Integration Tests
//!
//! Sub-modules are conditionally compiled based on feature flags.

#[path = "sync/circuit_breaker.rs"]
mod circuit_breaker;
#[path = "sync/types.rs"]
mod types;

#[cfg(feature = "fulltext-search")]
#[path = "sync/batch_processor.rs"]
mod batch_processor;
#[cfg(feature = "fulltext-search")]
#[path = "sync/comprehensive.rs"]
mod comprehensive;
#[cfg(feature = "fulltext-search")]
#[path = "sync/dlq_recovery.rs"]
mod dlq_recovery;
#[cfg(feature = "fulltext-search")]
#[path = "sync/edge.rs"]
mod edge;
#[cfg(feature = "fulltext-search")]
#[path = "sync/fault_tolerance.rs"]
mod fault_tolerance;
#[cfg(feature = "fulltext-search")]
#[path = "sync/integration.rs"]
mod integration;
#[cfg(feature = "fulltext-search")]
#[path = "sync/recovery_e2e.rs"]
mod recovery_e2e;
#[cfg(feature = "fulltext-search")]
#[path = "sync/transaction_basic.rs"]
mod transaction_basic;
#[cfg(feature = "fulltext-search")]
#[path = "sync/two_pc_protocol.rs"]
mod two_pc_protocol;

#[cfg(feature = "vector-qdrant")]
#[path = "sync/vector_sync.rs"]
mod vector_sync;
#[cfg(feature = "vector-qdrant")]
#[path = "sync/vector_transaction.rs"]
mod vector_transaction;
