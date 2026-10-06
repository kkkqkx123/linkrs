//! gRPC Service Module
//!
//! Provides an interface to GraphDB services based on the gRPC protocol.
//!
//! The whole transport boundary returns `Result<T, tonic::Status>`: the error
//! type and its size are fixed by the tonic trait contracts (generated and
//! implemented here), so `clippy::result_large_err` cannot be satisfied by
//! boxing without breaking those traits.
#![allow(clippy::result_large_err)]

pub mod batch;
pub mod bootstrap;
pub mod config;
pub mod convert;
pub mod error;
pub mod function;
pub mod migration;
pub mod query;
pub mod schema;
pub mod server;
pub mod service;
pub mod session;
pub mod statistics;
pub mod transaction;
pub mod vector;

// Proto module will be generated at compile time
pub mod proto {
    tonic::include_proto!("graphdb");
}

pub use bootstrap::{run_server, run_server_with_grpc_service};
pub use service::GraphDBService;
