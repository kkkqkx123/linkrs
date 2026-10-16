//! Session Management Module
//!
//! Provide the concept of a "session" as the context in which queries are executed.
//!
//! The original single `session.rs` (≈2065 lines) mixed ten-plus responsibilities
//! in one `impl` block. It has been split into the submodules below; this file
//! stays the module root and only owns the type definitions, the constructor,
//! `Drop`, the unit tests, and the submodule declarations. All behaviour lives
//! in `session/*`.

use crate::api_core::{QueryApi, SchemaApi};
use crate::embedded::hooks::HookBus;
use crate::session_variables::SessionVariables;
use linkrs_core::SessionStatistics;
use linkrs_metrics::StatsManager;
use linkrs_query::executor::expression::functions::FunctionRegistry;
use linkrs_storage::StorageClient;
use linkrs_sync::SyncManager;
use linkrs_transaction::TransactionId;
use linkrs_transaction::TransactionManager;
use parking_lot::RwLock;
use std::sync::Arc;
#[cfg(feature = "vector")]
use tokio::runtime::Runtime;

/// Session – Execution Context
///
/// A session is the basic unit for the execution of queries, and it maintains contextual information such as the current graph space and the transaction status.
///
/// # Examples
///
/// ```rust
/// use linkrs_api::embedded::{GraphDatabase, DatabaseConfig};
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let db = GraphDatabase::open("my_db")?;
/// let mut session = db.session()?;
///
/// // Switch to the image space
/// session.use_space("test_space")?;
///
/// // Execute the query
/// let result = session.execute("MATCH (n) RETURN n")?;
///
/// // Using a transaction
/// let txn = session.begin_transaction()?;
/// txn.execute("CREATE TAG user(name string)")?;
/// txn.commit()?;
/// # Ok(())
/// # }
/// ```
pub struct Session<S: StorageClient + Clone + Send + Sync + 'static> {
    db: Arc<GraphDatabaseInner<S>>,
    space_id: Arc<RwLock<Option<u64>>>,
    space_name: Arc<RwLock<Option<String>>>,
    auto_commit: bool,
    /// Session-level transaction binding started through a text
    /// `BEGIN` statement (the explicit `Transaction` handle API is
    /// independent of this).
    current_transaction: Arc<RwLock<Option<TransactionId>>>,
    /// Session-level change statistics
    statistics: SessionStatistics,
    /// Session-level function registry
    function_registry: Arc<RwLock<FunctionRegistry>>,
    /// Session-scoped user variables (`$name`) with transaction overlay.
    session_variables: Arc<crate::session_variables::SessionVariables>,
    /// Cooperative interrupt flag: set by `interrupt()` or the C-API
    /// `linkrs_connection_interrupt`; `execute*` entry points fail fast
    /// when set. Queries already running need the kill path below instead.
    interrupted: Arc<std::sync::atomic::AtomicBool>,
}

/// Internal structure of the database, used for sharing data between Session and GraphDatabase
pub(crate) struct GraphDatabaseInner<S: StorageClient + Clone + Send + Sync + 'static> {
    pub(crate) query_api: Arc<RwLock<QueryApi<S>>>,
    pub(crate) schema_api: SchemaApi<S>,
    pub(crate) txn_manager: Arc<TransactionManager>,
    pub(crate) storage: Arc<RwLock<S>>,
    /// Read by the vector search paths; kept alive for every other
    /// feature set so the sync subsystem outlives database assembly.
    #[cfg_attr(not(feature = "vector"), allow(dead_code))]
    pub(crate) sync_manager: Option<Arc<SyncManager>>,
    pub(crate) stats_manager: Arc<StatsManager>,
    /// Central event-hook facade.
    pub(crate) hooks: HookBus,
    /// Tokio runtime for vector operations in embedded mode.
    /// Kept alive (never read) for the lifetime of the database: the
    /// vector coordinator holds a runtime handle, and dropping the
    /// runtime would shut it down underneath the coordinator.
    #[cfg(feature = "vector")]
    #[allow(dead_code)]
    pub(crate) vector_runtime: Arc<Runtime>,
    /// Whether the database was opened read-only. Sessions consult this
    /// flag to reject mutating statements up front.
    pub(crate) read_only: bool,
}

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Create a new session.
    pub(crate) fn new(db: Arc<GraphDatabaseInner<S>>) -> Self {
        Self {
            db,
            space_id: Arc::new(RwLock::new(None)),
            space_name: Arc::new(RwLock::new(None)),
            auto_commit: true,
            current_transaction: Arc::new(RwLock::new(None)),
            statistics: SessionStatistics::new(),
            function_registry: Arc::new(RwLock::new(FunctionRegistry::new())),
            session_variables: Arc::new(SessionVariables::new()),
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

impl<S: StorageClient + Clone + Send + Sync + 'static> Drop for Session<S> {
    fn drop(&mut self) {
        // No special cleanup is required when the session is discarded.
        // Because all transactions are managed through the Transaction object, and Transactions have their own Drop implementation
        // Just logging here for debugging purposes
        log::debug!(
            "Session released, current graph space: {:?}",
            self.space_name.read()
        );
    }
}

pub(crate) mod create_as;
pub(crate) mod execution;
pub(crate) mod extension;
pub(crate) mod state;
pub(crate) mod transactions;
#[cfg(feature = "vector")]
pub(crate) mod vector;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::database::GraphDatabase;
    use linkrs_storage::{GraphStorage, MockStorage};

    #[test]
    fn create_as_detection_follows_the_parsed_target() {
        assert!(Session::<GraphStorage>::parse_create_as(
            "CREATE TAG adults AS (MATCH (n) RETURN n)"
        )
        .is_some());
        assert!(Session::<GraphStorage>::parse_create_as(
            "CREATE EDGE worked AS (MATCH (n) RETURN n)"
        )
        .is_some());

        // Ordinary CREATE statements are not CREATE ... AS.
        assert!(Session::<GraphStorage>::parse_create_as("CREATE TAG t(name string)").is_none());
        // Text that merely contains "AS (" inside a literal is not gated in
        // and never reaches the parser.
        assert!(Session::<GraphStorage>::parse_create_as("MATCH (n) RETURN 'AS ('").is_none());
        assert!(!Session::<GraphStorage>::starts_with_create(
            "MATCH (n) RETURN n"
        ));
    }

    #[test]
    fn interrupt_flag_fails_execute_fast_and_clears() {
        let db = GraphDatabase::<MockStorage>::open_test().expect("test database opens");
        let session = db.session().expect("session creates");
        assert!(!session.is_interrupted());

        session.interrupt();
        assert!(session.is_interrupted());
        let err = session
            .execute("MATCH (n) RETURN n")
            .expect_err("interrupted session must fail fast");
        assert!(
            err.to_string().contains("interrupted"),
            "unexpected error: {err}"
        );

        session.clear_interrupt();
        assert!(!session.is_interrupted());
    }
}
