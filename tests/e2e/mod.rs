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

pub mod common;
pub mod data_driven;
pub mod extended_types;
pub mod optimizer;
pub mod schema_manager;
pub mod social_network;
