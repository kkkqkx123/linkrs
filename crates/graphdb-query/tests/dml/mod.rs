//! Data Manipulation Language (DML) Integration Tests
//!
//! Test coverage:
//! - INSERT VERTEX - Insert vertex data
//! - INSERT EDGE - Insert edge data
//! - UPDATE - Update properties
//! - DELETE - Delete vertices and edges
//! - UPSERT - Insert or update
//! - MERGE - Merge operation

mod batch_operations;
mod common;
mod constraints;
mod delete;
mod insert_edge;
mod insert_vertex;
mod update;
mod upsert;
