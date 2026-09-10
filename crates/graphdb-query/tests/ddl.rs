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

#[path = "ddl/attach_detach.rs"]
mod attach_detach;
mod common;
#[path = "ddl/constraints.rs"]
mod constraints;
#[path = "ddl/edge_alter.rs"]
mod edge_alter;
#[path = "ddl/edge_basic.rs"]
mod edge_basic;
#[path = "ddl/macro.rs"]
mod macro_;
#[path = "ddl/schema_evolution.rs"]
mod schema_evolution;
#[path = "ddl/show_catalog.rs"]
mod show_catalog;
#[path = "ddl/tag_alter.rs"]
mod tag_alter;
#[path = "ddl/tag_basic.rs"]
mod tag_basic;
#[path = "ddl/type_alias.rs"]
mod type_alias;
#[path = "ddl/type_system.rs"]
mod type_system;
