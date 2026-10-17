//! Data Definition Language (DDL) Integration Tests
//!
//! Test coverage:
//! - CREATE TAG - Create vertex tag
//! - CREATE EDGE - Create edge type
//! - CREATE SPACE - Create graph space
//! - CREATE INDEX - Create tag/edge indexes
//! - ALTER TAG - Modify vertex tag
//! - ALTER EDGE - Modify edge type
//! - DROP TAG - Delete vertex tag
//! - DROP EDGE - Delete edge type
//! - DROP SPACE / DROP GRAPH - Delete graph space
//! - DROP INDEX - Delete tag/edge indexes
//! - DESC - Describe schema objects
//! - Constraints - DEFAULT, NOT NULL

#[path = "ddl/attach_detach.rs"]
mod attach_detach;
mod common;
#[path = "ddl/constraints.rs"]
mod constraints;
#[path = "ddl/contract_gaps.rs"]
mod contract_gaps;
#[path = "ddl/edge_alter.rs"]
mod edge_alter;
#[path = "ddl/edge_basic.rs"]
mod edge_basic;
#[path = "ddl/index_basic.rs"]
mod index_basic;
#[path = "ddl/macro.rs"]
mod macro_;
#[path = "ddl/schema_evolution.rs"]
mod schema_evolution;
#[path = "ddl/sequence.rs"]
mod sequence;
#[path = "ddl/show_catalog.rs"]
mod show_catalog;
#[path = "ddl/show_create.rs"]
mod show_create;
#[path = "ddl/space_basic.rs"]
mod space_basic;
#[path = "ddl/tag_alter.rs"]
mod tag_alter;
#[path = "ddl/tag_basic.rs"]
mod tag_basic;
#[path = "ddl/type_alias.rs"]
mod type_alias;
#[path = "ddl/type_system.rs"]
mod type_system;
