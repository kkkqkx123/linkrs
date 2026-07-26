//! gRPC Service Module
//!
//! Provides an interface to GraphDB services based on the gRPC protocol.

pub mod server;

// Proto module will be generated at compile time
pub mod proto {
    tonic::include_proto!("graphdb");
}

pub use server::{run_server, run_server_with_grpc_service, GraphDBService};
