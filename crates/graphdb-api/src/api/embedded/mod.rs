//! Embedded API module
//!
//! Provide an embedded GraphDB interface for standalone use, with a similar usage approach to SQLite.
//!
//! # Get started quickly
//!
//! ```rust
//! use graphdb::api::embedded::{GraphDatabase, DatabaseConfig};
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
pub mod result;
pub mod session;
pub mod statistics;
pub mod transaction;

// C API module
pub mod c_api;

// Re-export the main types
pub use batch::{BatchConfig, BatchError, BatchInserter, BatchItemType, BatchResult};
pub use busy_handler::{BusyConfig, BusyHandler, BusyResult};
pub use config::{DatabaseConfig, SyncMode};
pub use database::GraphDatabase;
pub use result::{QueryResult, ResultMetadata, Row};
pub use session::Session;
pub use statistics::QueryStatistics;
pub use transaction::{Transaction, TransactionConfig, TransactionInfo};

// Re-export SessionStatistics from core
pub use crate::core::SessionStatistics;

// C API re-export
pub use c_api::{
    error::graphdb_error_code_t,
    statistics::SessionStatistics as CApiSessionStatistics,
    types::{
        graphdb_batch_t, graphdb_config_t, graphdb_result_t, graphdb_session_t, graphdb_string_t,
        graphdb_t, graphdb_txn_t, graphdb_value_data_t, graphdb_value_t, graphdb_value_type_t,
    },
};

// Error type
pub use crate::api::core::CoreError as EmbeddedError;
