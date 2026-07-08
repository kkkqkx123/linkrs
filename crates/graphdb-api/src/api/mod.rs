//! The GraphDB API module
//!
//! Provide multiple access methods:
//! - "core" — core API independent of the transport layer
//! - "server" — network service API (HTTP/gRPC)
//! - "embedded" — standalone embedded API

pub mod core;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "embedded")]
pub mod embedded;

// ── Core re-exports ──────────────────────────────────────────────
pub use core::{CoreError, CoreResult, QueryApi, SchemaApi, SyncApi};

#[cfg(feature = "qdrant")]
pub use core::{VectorApi, VectorSearchResult};

// ── Server re-exports ────────────────────────────────────────────
#[cfg(feature = "server")]
pub use server::{session, HttpServer};

#[cfg(feature = "embedded")]
pub use embedded::GraphDatabase;

// ── Private implementation modules ───────────────────────────────
#[cfg(feature = "server")]
mod startup;

#[cfg(feature = "server")]
mod http_server;

mod shutdown;

// ── Public API re-exports ────────────────────────────────────────
#[cfg(feature = "server")]
pub use startup::{execute_query, start_service, start_service_with_config};

#[cfg(feature = "server")]
pub use http_server::start_http_server;

#[cfg(all(feature = "server", feature = "grpc"))]
pub use http_server::start_http_and_grpc_servers;

pub use shutdown::shutdown_signal;
