//! Wire contract crate: DTOs shared between the network service
//! (`graphdb-server`) and the CLI.
//!
//! The HTTP contract has a single source of truth here: the server
//! serializes these types and the CLI deserializes them (and vice versa for
//! requests), eliminating field-by-field mirror DTOs in both crates.
//!
//! Dependency surface is deliberately minimal (`serde` + `serde_json`) so the
//! crate stays usable from both sides without dragging in axum/tonic.

pub mod batch;
pub mod meta;
pub mod query;
pub mod schema;
