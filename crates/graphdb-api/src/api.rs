//! The GraphDB API module
//!
//! Provides the transport-independent core API consumed by the network
//! service layer (`graphdb-server`) and library users.

pub mod core;

// Transaction-aware session variable store shared by the server and
// embedded session implementations.
pub mod session_variables;

#[cfg(feature = "embedded")]
pub mod embedded;

// ── Core re-exports ──────────────────────────────────────────────
pub use core::{CoreError, CoreResult, QueryApi, SchemaApi, SyncApi};

#[cfg(feature = "qdrant")]
pub use core::{VectorApi, VectorSearchResult};

#[cfg(feature = "embedded")]
pub use embedded::GraphDatabase;
