//! GraphDB network service layer (HTTP/gRPC).
//!
//! Standalone server crate decoupled from the programmatic API
//! (`graphdb-api`) and the wire contract (`graphdb-wire`). The service
//! exposes the core API over HTTP/gRPC and the web management interface.

pub use graphdb_api as api;
pub use graphdb_config as config;
pub use graphdb_core as core;
pub use graphdb_query as query;

pub mod storage {
    pub use graphdb_storage::*;

    #[cfg(test)]
    pub use graphdb_storage::MockStorage;
}

// Network service modules (moved from `graphdb-api::api::server`).
pub mod http_server;
mod shutdown;
pub mod startup;
pub mod value;

#[cfg(feature = "vector")]
pub mod vector_metrics;

// Server sub-modules
pub mod auth;
pub mod batch;
pub mod client;
pub mod graph_service;
#[cfg(feature = "grpc")]
pub mod grpc;
pub mod http;
pub mod permission;
pub mod session;
pub mod web;

pub use auth::{Authenticator, PasswordAuthenticator};
pub use batch::BatchManager;
pub use client::{ClientSession, Session};
pub use graph_service::GraphService;
#[cfg(feature = "grpc")]
pub use grpc::{run_server, GraphDBService};
pub use http::HttpServer;
pub use permission::{Permission, PermissionChecker, PermissionManager, RoleType};
pub use session::GraphSessionManager;
pub use web::WebState;

#[cfg(feature = "grpc")]
pub use http_server::start_http_and_grpc_servers;
pub use http_server::start_http_server;
pub use shutdown::shutdown_signal;
pub use startup::{execute_query, start_service, start_service_with_config};
