#![cfg(feature = "fulltext")]

#[path = "fulltext_tests/advanced_queries.rs"]
mod advanced_queries;
#[path = "fulltext_tests/basic.rs"]
mod basic;
#[path = "fulltext_tests/common.rs"]
mod common;
#[path = "fulltext_tests/concurrent.rs"]
mod concurrent;
#[path = "fulltext_tests/edge_cases.rs"]
mod edge_cases;
#[path = "fulltext_tests/persistence.rs"]
mod persistence;
#[path = "fulltext_tests/sync.rs"]
mod sync;
#[path = "fulltext_tests/transaction.rs"]
mod transaction;
