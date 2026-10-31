pub use linkrs_api as api;
pub use linkrs_config as config;
pub use linkrs_core as core;
pub use linkrs_fulltext as search;
pub use linkrs_migration as migration;
pub use linkrs_query as query;
pub use linkrs_storage as storage;
pub use linkrs_sync as sync;
pub use linkrs_transaction as transaction;

#[cfg(feature = "c_api")]
pub mod c_api;

/// Test-only helpers shared by unit and integration tests. Compiled for
/// in-crate unit tests and for integration tests via the `test-utils`
/// feature (enabled by the self dev-dependency); never part of the
/// released library API.
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
