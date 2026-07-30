//! Transaction Module Integration Tests
//!
//! Test coverage:
//! - Basic lifecycle - begin, commit, rollback
//! - Vertex operations - insert, update, delete
//! - Edge operations - create, delete, properties
//! - Complex operations - multiple operations, cascading
//! - Concurrent transactions - read-only concurrency, write exclusivity
//! - Timeout handling - transaction timeout, query timeout, statement timeout, idle timeout
//! - Savepoints - create, rollback, multiple, find by name
//! - Durability levels - immediate, none
//! - Statistics - transaction stats, cleanup
//! - Retry mechanism - execute_with_retry, retryable vs non-retryable errors
//! - Batch commit - commit multiple transactions
//! - Metrics - transaction metrics collection
//! - Max concurrent - transaction limit enforcement
//! - Cleanup - expired transaction cleanup
//! - Shutdown - graceful shutdown with active transactions
//! - Transaction info - list active, get info by id
//! - HTTP API - BEGIN/COMMIT/ROLLBACK via HTTP API, concurrent HTTP requests, async/await pattern
//! - Deadlock prevention - verifies fix for spawn_blocking + block_on deadlock issue
//! - Rollback operations - operation log rollback for vertices and edges
//! - Error scenarios - various error conditions and edge cases
//! - Config options - transaction and manager configuration
//! - Recovery - timeout cleanup, shutdown, MVCC frontier advance
//! - Crash recovery - WAL recovery after simulated crash
//! - Storage integration - transaction integration with storage layer
//! - Edge advanced - advanced edge operations and patterns

#[path = "transaction/admission_timeout.rs"]
mod admission_timeout;
#[path = "transaction/advanced.rs"]
mod advanced;
#[path = "transaction/api_consistency.rs"]
mod api_consistency;
#[path = "transaction/basic.rs"]
mod basic;
#[path = "transaction/complex.rs"]
mod complex;
#[path = "transaction/concurrent.rs"]
mod concurrent;
#[path = "transaction/config_options.rs"]
mod config_options;
#[path = "transaction/deadlock_prevention.rs"]
mod deadlock_prevention;
#[path = "transaction/edge.rs"]
mod edge;
#[path = "transaction/edge_advanced.rs"]
mod edge_advanced;
#[path = "transaction/error_scenarios.rs"]
mod error_scenarios;
#[path = "transaction/http_api.rs"]
mod http_api;
#[path = "transaction/lifecycle.rs"]
mod lifecycle;
#[path = "transaction/recovery.rs"]
mod recovery;
#[path = "transaction/crash_recovery.rs"]
mod crash_recovery;
#[path = "transaction/rollback_operations.rs"]
mod rollback_operations;
#[path = "transaction/semantics.rs"]
mod semantics;
#[path = "transaction/storage_integration.rs"]
mod storage_integration;
#[path = "transaction/timeout.rs"]
mod timeout;
#[path = "transaction/vertex.rs"]
mod vertex;
