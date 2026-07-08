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
//! - Two-phase commit - distributed transaction coordination
//! - Error scenarios - various error conditions and edge cases
//! - Config options - transaction and manager configuration
//! - Storage integration - transaction integration with storage layer
//! - Edge advanced - advanced edge operations and patterns

mod advanced;
mod basic;
mod common;
mod complex;
mod concurrent;
mod config_options;
mod deadlock_prevention;
mod edge;
mod edge_advanced;
mod error_scenarios;
mod http_api;
mod rollback_operations;
mod storage_integration;
mod timeout;
mod two_phase_commit;
mod vertex;
mod write_lock_timeout;
