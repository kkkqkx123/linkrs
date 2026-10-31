//! Transport-neutral services shared by the HTTP and gRPC layers.
//!
//! Transport handlers stay thin: they only map their request/response
//! shapes and delegate the business logic to the modules below.

pub(crate) mod authz;
pub(crate) mod config;
#[cfg(feature = "fulltext")]
pub(crate) mod fulltext;
pub(crate) mod function;
pub(crate) mod migration_progress;
pub(crate) mod query;
pub(crate) mod rebuild;
pub(crate) mod schema_migration;
pub(crate) mod statistics;
#[cfg(feature = "vector")]
pub(crate) mod vector;
