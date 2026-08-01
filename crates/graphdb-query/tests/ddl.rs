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
#[path = "ddl/constraints.rs"]
mod constraints;
#[path = "ddl/edge_alter.rs"]
mod edge_alter;
#[path = "ddl/edge_basic.rs"]
mod edge_basic;
#[path = "ddl/schema_evolution.rs"]
mod schema_evolution;
#[path = "ddl/tag_alter.rs"]
mod tag_alter;
#[path = "ddl/tag_basic.rs"]
mod tag_basic;
