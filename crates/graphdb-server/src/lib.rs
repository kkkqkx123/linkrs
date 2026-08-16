//! GraphDB network service layer (HTTP/gRPC).
//!
//! Standalone server crate decoupled from the programmatic API
//! (`graphdb-api`) and the wire contract (`graphdb-wire`). The service
//! exposes the core API over HTTP/gRPC and the web management interface.

pub use graphdb_api::api;
pub use graphdb_config::config;
pub use graphdb_core::core;
pub use graphdb_core::utils;
pub use graphdb_query::query;
pub use graphdb_search::search;
pub use graphdb_sync::sync;
pub use graphdb_transaction::transaction;

pub mod storage {
    pub use graphdb_storage::storage::*;

    #[cfg(test)]
    pub use graphdb_storage::storage::MockStorage;
}

// Network service modules (moved from `graphdb-api::api::server`).
pub mod server;
pub mod startup;
pub mod http_server;
mod shutdown;
pub mod value;

pub use http_server::start_http_server;
#[cfg(feature = "grpc")]
pub use http_server::start_http_and_grpc_servers;
pub use shutdown::shutdown_signal;
pub use startup::{execute_query, start_service, start_service_with_config};
