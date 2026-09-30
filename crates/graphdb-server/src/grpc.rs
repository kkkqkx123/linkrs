//! gRPC Service Module
//!
//! Provides an interface to GraphDB services based on the gRPC protocol.
//!
//! The whole transport boundary returns `Result<T, tonic::Status>`: the error
//! type and its size are fixed by the tonic trait contracts (generated and
//! implemented here), so `clippy::result_large_err` cannot be satisfied by
//! boxing without breaking those traits.
#![allow(clippy::result_large_err)]

pub mod server;

// Proto module will be generated at compile time
pub mod proto {
    tonic::include_proto!("graphdb");
}

pub use server::{run_server, run_server_with_grpc_service, GraphDBService};
