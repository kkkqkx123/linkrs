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

#[path = "vector_tests/advanced_filters.rs"]
mod advanced_filters;
#[path = "vector_tests/basic.rs"]
mod basic;
#[path = "vector_tests/collection_config.rs"]
mod collection_config;
#[path = "vector_tests/common.rs"]
mod common;
#[path = "vector_tests/concurrent.rs"]
mod concurrent;
#[path = "vector_tests/edge_cases.rs"]
mod edge_cases;
#[path = "vector_tests/embedding.rs"]
mod embedding;
#[path = "vector_tests/operations.rs"]
mod operations;
#[path = "vector_tests/search.rs"]
mod search;
#[path = "vector_tests/search_mode.rs"]
mod search_mode;
