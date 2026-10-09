//! Embedded API module
//!
//! Provide an embedded Linkrs interface for standalone use, with a similar usage approach to SQLite.
//!
//! # Get started quickly
//!
//! ```rust
//! use linkrs_api::embedded::{GraphDatabase, DatabaseConfig};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
// Open the database
//! let db = GraphDatabase::open("my_database")?;
//!
// Create a session
//! let mut session = db.session()?;
//!
// Switch to the image space
//! session.use_space("test_space")?;
//!
// Execute the query
//! let result = session.execute("MATCH (n) RETURN n")?;
//!
// Using a transaction
//! let txn = session.begin_transaction()?;
//! txn.execute("CREATE TAG user(name string)")?;
//! txn.commit()?;
//!
// The database is automatically closed when the `db` variable goes out of scope.
//! # Ok(())
//! # }
//! ```

// Submodule
pub mod batch;
pub mod busy_handler;
pub mod config;
pub mod database;
pub mod hooks;
pub mod migration;
pub mod result;
pub mod session;
pub mod statistics;
pub mod transaction;

// C API module: compiled only for callers that ask for the FFI surface.
#[cfg(feature = "c_api")]
pub mod c_api;

// Re-export the main types
pub use batch::{BatchError, BatchInserter, BatchItemType, BatchResult};
pub use busy_handler::{BusyConfig, BusyHandler, BusyResult};
pub use config::{DatabaseConfig, SyncMode};
pub use database::GraphDatabase;
pub use hooks::HookBus;
pub use result::{QueryResult, ResultMetadata, Row};
pub use session::Session;
pub use statistics::QueryStatistics;
pub use transaction::{Transaction, TransactionConfig, TransactionInfo};

// Re-export SessionStatistics from core
pub use linkrs_core::SessionStatistics;

// C API re-export
#[cfg(feature = "c_api")]
pub use c_api::{
    error::linkrs_error_code_t,
    statistics::linkrs_session_statistics_t as CApiSessionStatistics,
    types::{
        linkrs_batch_t, linkrs_config_t, linkrs_result_t, linkrs_session_t, linkrs_string_t,
        linkrs_t, linkrs_txn_t, linkrs_value_data_t, linkrs_value_t, linkrs_value_type_t,
    },
};

// Error type
pub use crate::api_core::error::CoreError as EmbeddedError;
