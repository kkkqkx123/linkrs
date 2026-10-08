//! Statistical information about the HTTP processor.
//!
//! The handlers live in submodules grouped by responsibility; this file
//! only declares the modules and re-exports the public surface so existing
//! paths such as `handlers::statistics::session` keep working.
//!
//! - [`query`]: session, query, search and portrait statistics.
//! - [`system`]: system resource usage and host probes.
//! - [`database`]: database overview assembly.
//! - [`overview`]: aggregated monitoring overview.
//! - [`freeze`]: background freeze statistics and triggers.
//! - [`migration`]: migration counters.

pub mod database;
pub mod freeze;
pub mod migration;
pub mod overview;
pub mod query;
pub mod system;

pub use database::database;
pub use freeze::{freeze_stats, trigger_freeze};
pub use migration::migration;
pub use overview::overview;
pub use query::{queries, query_profile_detail, search, session, QueryStatsParams};
pub use system::system;
