//! Data Manipulation Language (DML) Integration Tests
//!
//! Test coverage:
//! - INSERT VERTEX - Insert vertex data
//! - INSERT EDGE - Insert edge data
//! - UPDATE - Update properties
//! - DELETE - Delete vertices and edges
//! - UPSERT - Insert or update
//! - MERGE - Merge operation

#[path = "dml/batch_operations.rs"]
mod batch_operations;
mod common;
#[path = "dml/constraints.rs"]
mod constraints;
#[path = "dml/delete.rs"]
mod delete;
#[path = "dml/insert_edge.rs"]
mod insert_edge;
#[path = "dml/insert_vertex.rs"]
mod insert_vertex;
#[path = "dml/update.rs"]
mod update;
#[path = "dml/upsert.rs"]
mod upsert;
