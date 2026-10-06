mod batch_exec;
mod command;
mod context;
mod cursor;
mod error;
mod explain;
mod factory;
mod guard;
mod query;
mod session;
mod streaming;

pub use context::QueryExecutionContext;
pub use error::GraphServiceError;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

#[cfg(feature = "vector")]
use graphdb_api::api_core::VectorApi;
use graphdb_api::api_core::{QueryApi, SyncApi};
use graphdb_metrics::StatsManager;
use graphdb_query::query_manager::QueryManager;
use graphdb_transaction::TransactionManager;

use crate::auth::PasswordAuthenticator;
use crate::permission::PermissionManager;
use crate::query::executor::streaming::pool::SharedScheduler;
use crate::query::executor::streaming::query_registry::QueryRegistry;
use crate::session::GraphSessionManager;
use crate::storage::StorageClient;

pub struct GraphService<S: StorageClient + Clone + 'static> {
    session_manager: Arc<GraphSessionManager>,
    query_api: Arc<parking_lot::RwLock<QueryApi<S>>>,
    authenticator: PasswordAuthenticator,
    permission_manager: Arc<PermissionManager>,
    pub stats_manager: Arc<StatsManager>,
    storage: Arc<S>,
    #[cfg(feature = "vector")]
    vector_api: Option<Arc<VectorApi>>,
    sync_api: Option<Arc<SyncApi>>,

    transaction_manager: Option<Arc<TransactionManager>>,

    /// Engine-level shared scheduler, created once at startup.
    shared_scheduler: Arc<SharedScheduler>,
    /// Process-level query registry, created once at startup.
    query_registry: Arc<QueryRegistry>,
    /// Process-level query manager.
    query_manager: Arc<QueryManager>,

    /// Row interval for query-progress notifications (0 disables emission).
    progress_rows_interval: u64,

    /// Monotonically increasing query ID counter (server-assigned, not hash-based).
    next_query_id: AtomicU64,
}

#[cfg(test)]
mod tests {
    use super::GraphService;
    use crate::storage::MockStorage;

    #[test]
    fn transaction_command_classification() {
        let begin = GraphService::<MockStorage>::parse_command("BEGIN READ ONLY")
            .expect("BEGIN should classify")
            .expect("BEGIN should be a command");
        assert!(matches!(
            begin.ast.stmt(),
            crate::query::parser::ast::Stmt::BeginTransaction(ref s) if s.read_only == Some(true)
        ));

        let commit = GraphService::<MockStorage>::parse_command("COMMIT")
            .expect("COMMIT should classify")
            .expect("COMMIT should be a command");
        assert!(matches!(
            commit.ast.stmt(),
            crate::query::parser::ast::Stmt::CommitTransaction(_)
        ));

        let rollback = GraphService::<MockStorage>::parse_command("ROLLBACK TO sp1")
            .expect("ROLLBACK TO should classify")
            .expect("ROLLBACK TO should be a command");
        assert!(matches!(
            rollback.ast.stmt(),
            crate::query::parser::ast::Stmt::RollbackTransaction(ref s)
                if s.savepoint_name.as_deref() == Some("sp1")
        ));

        let savepoint = GraphService::<MockStorage>::parse_command("SAVEPOINT sp1")
            .expect("SAVEPOINT should classify")
            .expect("SAVEPOINT should be a command");
        assert!(matches!(
            savepoint.ast.stmt(),
            crate::query::parser::ast::Stmt::Savepoint(ref s) if s.name == "sp1"
        ));

        let release = GraphService::<MockStorage>::parse_command("RELEASE SAVEPOINT sp1")
            .expect("RELEASE SAVEPOINT should classify")
            .expect("RELEASE SAVEPOINT should be a command");
        assert!(matches!(
            release.ast.stmt(),
            crate::query::parser::ast::Stmt::ReleaseSavepoint(ref s) if s.name == "sp1"
        ));

        let let_stmt = GraphService::<MockStorage>::parse_command("LET $x = 1 + 2")
            .expect("LET should classify")
            .expect("LET should be a command");
        assert!(matches!(
            let_stmt.ast.stmt(),
            crate::query::parser::ast::Stmt::AssignVariable(ref s) if s.name == "x"
        ));

        assert!(
            GraphService::<MockStorage>::parse_command("MATCH (n) RETURN n")
                .unwrap()
                .is_none()
        );
        assert!(GraphService::<MockStorage>::parse_command("COMMIT junk")
            .unwrap()
            .is_none());

        let err =
            GraphService::<MockStorage>::parse_command("LET $x").expect_err("LET $x must fail");
        assert!(
            err.message().contains("LET requires an assignment"),
            "unexpected error: {}",
            err
        );

        let bare_let =
            GraphService::<MockStorage>::parse_command("LET").expect_err("bare LET must fail");
        assert!(
            bare_let.message().contains("Invalid session variable name"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn config_statements_are_classified_for_streaming_guard() {
        assert!(GraphService::<MockStorage>::is_config_statement(
            "UPDATE CONFIGS SET a = 1"
        ));
        assert!(GraphService::<MockStorage>::is_config_statement(
            "  show configs database"
        ));
        assert!(!GraphService::<MockStorage>::is_config_statement(
            "MATCH (n) RETURN n"
        ));
        assert!(!GraphService::<MockStorage>::is_config_statement(
            "UPDATE 1 ON Person SET age = 2"
        ));
    }

    #[test]
    fn root_estimate_takes_the_last_marker() {
        let plan = "info est_rows:10,other:1\nmore est_rows:250\ntail without marker";
        assert_eq!(
            GraphService::<MockStorage>::extract_root_estimate(plan),
            Some(250)
        );
    }

    #[test]
    fn root_estimate_absent_without_markers() {
        assert_eq!(
            GraphService::<MockStorage>::extract_root_estimate("no estimates here"),
            None
        );
        assert_eq!(
            GraphService::<MockStorage>::extract_root_estimate("est_rows:"),
            None
        );
    }
}
