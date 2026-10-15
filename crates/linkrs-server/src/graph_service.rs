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
use linkrs_api::api_core::VectorApi;
use linkrs_api::api_core::{QueryApi, SyncApi};
use linkrs_metrics::StatsManager;
use linkrs_query::query_manager::QueryManager;
use linkrs_transaction::TransactionManager;

use crate::auth::PasswordAuthenticator;
use crate::config::BootstrapConfig;
use crate::config::SecurityConfig;
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

    /// Bootstrap-time configuration used for single-user mode and other
    /// local-deployment toggles.
    bootstrap_config: BootstrapConfig,

    /// Password-policy and related security settings snapshotted at
    /// startup. All plaintext-password write paths validate against it.
    security_config: SecurityConfig,
}

#[cfg(test)]
mod tests {
    use super::GraphService;
    use crate::storage::{GraphStorage, MockStorage};
    use linkrs_core::types::{SpaceInfo, SpaceSummary};
    use std::sync::Arc;

    /// In-memory storage with a single space, the GraphService built over it,
    /// and that space's identity for role grants.
    struct MaintenanceTestService {
        service: Arc<GraphService<GraphStorage>>,
        space: SpaceSummary,
        space_id: i64,
    }

    impl MaintenanceTestService {
        async fn new() -> Self {
            use crate::storage::{PropertyGraphConfig, StorageReader, StorageSchemaOps};

            let storage = Arc::new(
                GraphStorage::new_with_config(PropertyGraphConfig::test())
                    .expect("in-memory storage"),
            );
            {
                let mut handle = (*storage).clone();
                let mut space = SpaceInfo::new("maint_space".to_string())
                    .with_vid_type(linkrs_core::DataType::BigInt);
                StorageSchemaOps::create_space(&mut handle, &mut space).expect("create space");
            }
            let space = storage
                .list_spaces()
                .expect("list spaces")
                .into_iter()
                .next()
                .expect("space must exist");
            let space_id = space.space_id as i64;
            let service = GraphService::<GraphStorage>::new_for_test(
                crate::config::Config::default(),
                storage,
            )
            .await;
            Self {
                service,
                space: SpaceSummary::from(&space),
                space_id,
            }
        }

        /// Create a session for `username` holding `role` in the test space.
        async fn session_with_role(&self, username: &str, role: linkrs_core::RoleType) -> i64 {
            self.service
                .permission_manager
                .grant_role(username, self.space_id, role)
                .expect("grant role");
            let session = self
                .service
                .get_session_manager()
                .create_session(username.to_string(), "127.0.0.1".to_string())
                .await
                .expect("create session");
            session.set_space(self.space.clone());
            session.id()
        }
    }

    #[tokio::test]
    async fn maintenance_statements_are_classified_above_read_only() {
        let fixture = MaintenanceTestService::new().await;
        let service = &fixture.service;

        // Maintenance statements must never inherit the read-only default:
        // `PURGE EXPIRED` reclaims storage (schema-level), `CHECKPOINT`
        // forces a snapshot (write-level).
        assert_eq!(
            service.extract_permission_from_statement("PURGE EXPIRED IN maint_space"),
            linkrs_core::Permission::Schema
        );
        assert_eq!(
            service.extract_permission_from_statement("purge expired in maint_space"),
            linkrs_core::Permission::Schema
        );
        assert_eq!(
            service.extract_permission_from_statement("CHECKPOINT"),
            linkrs_core::Permission::Write
        );
        assert_eq!(
            service.extract_permission_from_statement("SELECT 1"),
            linkrs_core::Permission::Read
        );
    }

    #[tokio::test]
    async fn purge_expired_requires_maintenance_permission_on_every_entry() {
        let fixture = MaintenanceTestService::new().await;
        let stmt = "PURGE EXPIRED IN maint_space";

        let guest = fixture
            .session_with_role("guest_user", linkrs_core::RoleType::Guest)
            .await;
        let writer = fixture
            .session_with_role("writer_user", linkrs_core::RoleType::User)
            .await;
        let dba = fixture
            .session_with_role("dba_user", linkrs_core::RoleType::Dba)
            .await;

        // Read-only and plain write roles are rejected on the unary entry.
        for session_id in [guest, writer] {
            let error = fixture
                .service
                .execute(session_id, stmt)
                .await
                .expect_err("role without maintenance rights must be rejected");
            assert!(
                error.message().contains("Permission check failed"),
                "unexpected error: {}",
                error
            );
        }
        // The same rejection happens on the streaming entry.
        let stream_error = match fixture.service.execute_stream(guest, stmt).await {
            Ok(_) => panic!("read-only role must be rejected on the streaming entry"),
            Err(error) => error,
        };
        assert!(
            stream_error.message().contains("Permission check failed"),
            "unexpected error: {}",
            stream_error
        );

        // The maintenance role executes the purge.
        fixture
            .service
            .execute(dba, stmt)
            .await
            .expect("dba role runs the purge");
        fixture
            .service
            .execute_stream(dba, stmt)
            .await
            .expect("dba role runs the purge on the streaming entry");
    }

    #[tokio::test]
    async fn checkpoint_requires_write_permission_on_every_entry() {
        let fixture = MaintenanceTestService::new().await;

        let guest = fixture
            .session_with_role("guest_user", linkrs_core::RoleType::Guest)
            .await;
        let writer = fixture
            .session_with_role("writer_user", linkrs_core::RoleType::User)
            .await;

        let unary_error = fixture
            .service
            .execute(guest, "CHECKPOINT")
            .await
            .expect_err("read-only role must be rejected");
        assert!(
            unary_error.message().contains("Permission check failed"),
            "unexpected error: {}",
            unary_error
        );
        let stream_error = match fixture.service.execute_stream(guest, "CHECKPOINT").await {
            Ok(_) => panic!("read-only role must be rejected on the streaming entry"),
            Err(error) => error,
        };
        assert!(
            stream_error.message().contains("Permission check failed"),
            "unexpected error: {}",
            stream_error
        );

        fixture
            .service
            .execute(writer, "CHECKPOINT")
            .await
            .expect("write role runs the checkpoint");
        fixture
            .service
            .execute_stream(writer, "CHECKPOINT")
            .await
            .expect("write role runs the checkpoint on the streaming entry");
    }

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
