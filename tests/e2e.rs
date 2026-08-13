//! E2E Test Suite for GraphDB
//!
//! This library provides all E2E tests for GraphDB.
//! Run with: cargo test --test integration_e2e
//!
//! Tests are organized by functionality:
//! - common: Shared test utilities
//! - social_network: Basic graph operations
//! - schema_manager: Schema management
//! - optimizer: Query optimization
//! - extended_types: Extended type support (geography, vector, fulltext)

#[path = "e2e/common.rs"]
pub mod common;
#[path = "e2e/data_driven.rs"]
pub mod data_driven;
#[path = "e2e/extended_types.rs"]
pub mod extended_types;
#[path = "e2e/optimizer.rs"]
pub mod optimizer;
#[path = "e2e/schema_manager.rs"]
pub mod schema_manager;
#[path = "e2e/session_transaction.rs"]
pub mod session_transaction;
#[path = "e2e/social_network.rs"]
pub mod social_network;
#[path = "e2e/subquery.rs"]
pub mod subquery;
