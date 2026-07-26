//! Vector Search Module Integration Tests
//!
//! Test coverage:
//! - Basic CRUD - create index, drop index, insert, update, delete, search
//! - Vector operations - single insert, batch insert, delete, search
//! - Search functionality - similarity search, filtered search, threshold search
//! - Concurrent operations - concurrent inserts, searches, mixed operations
//! - Edge cases - empty vector, dimension mismatch, invalid operations
//! - Error handling - index not found, duplicate creation, invalid queries
//! - Multi-space isolation - space isolation for vector indexes
//! - Performance - basic performance tests for vector operations

mod advanced_filters;
mod basic;
mod collection_config;
mod common;
mod concurrent;
mod edge_cases;
mod embedding;
mod operations;
mod search;
mod search_mode;
