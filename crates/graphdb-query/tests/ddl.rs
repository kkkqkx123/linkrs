//! Data Definition Language (DDL) Integration Tests
//!
//! Test coverage:
//! - CREATE TAG - Create vertex tag
//! - CREATE EDGE - Create edge type
//! - ALTER TAG - Modify vertex tag
//! - ALTER EDGE - Modify edge type
//! - DROP TAG - Delete vertex tag
//! - DROP EDGE - Delete edge type
//! - DESC - Describe schema objects
//! - Constraints - DEFAULT, NOT NULL

mod common;
mod constraints;
mod edge_alter;
mod edge_basic;
mod schema_evolution;
mod tag_alter;
mod tag_basic;
