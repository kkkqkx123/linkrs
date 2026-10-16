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
        use crate::graph_service::GraphService as TestService;
        use crate::storage::MockStorage;

        fn permission(text: &str) -> Option<linkrs_core::Permission> {
            let stmt = TestService::<MockStorage>::parse_stmt(text)?;
            TestService::<MockStorage>::permission_for_stmt(&stmt)
        }

        // Maintenance statements must never inherit the read-only default:
        // `PURGE EXPIRED` reclaims storage (schema-level), `CHECKPOINT`
        // forces a snapshot (write-level).
        assert_eq!(
            permission("PURGE EXPIRED IN maint_space"),
            Some(linkrs_core::Permission::Schema)
        );
        assert_eq!(
            permission("purge expired in maint_space"),
            Some(linkrs_core::Permission::Schema)
        );
        assert_eq!(
            permission("CHECKPOINT"),
            Some(linkrs_core::Permission::Write)
        );
        assert_eq!(
            permission("SHOW SESSIONS"),
            Some(linkrs_core::Permission::Admin)
        );
        assert_eq!(
            permission("SHOW QUERIES"),
            Some(linkrs_core::Permission::Read)
        );
        assert_eq!(
            permission("KILL QUERY 1, 2"),
            Some(linkrs_core::Permission::Admin)
        );
        assert_eq!(
            permission("MATCH (n) RETURN n"),
            Some(linkrs_core::Permission::Read)
        );
        assert_eq!(
            permission("UPDATE 1 ON Person SET age = 2"),
            Some(linkrs_core::Permission::Write)
        );
        assert_eq!(
            permission("CREATE TAG t (id: INT)"),
            Some(linkrs_core::Permission::Schema)
        );
        assert_eq!(
            permission("CREATE SPACE s"),
            Some(linkrs_core::Permission::Admin)
        );
        assert_eq!(permission("CREATE USER u WITH PASSWORD 'x'"), None);
        assert_eq!(permission("CREATEUSER u WITH PASSWORD 'x'"), None);
        assert_eq!(permission("SELECT 1"), None);
    }

    #[tokio::test]
    async fn user_management_spellings_share_one_gate() {
        let fixture = MaintenanceTestService::new().await;
        let service = &fixture.service;
        let guest = fixture
            .session_with_role("gate_guest", linkrs_core::RoleType::Guest)
            .await;
        for stmt in [
            "CREATE USER gate_u WITH PASSWORD 'secret123'",
            "CREATEUSER gate_u WITH PASSWORD 'secret123'",
            "CREATE  USER gate_u WITH PASSWORD 'secret123'",
            "CHANGE PASSWORD guest_other 'old12345' TO 'new12345'",
            "CHANGEPASSWORD guest_other 'old12345' TO 'new12345'",
        ] {
            let error = service
                .execute(guest, stmt)
                .await
                .expect_err("guest must not pass the user gate");
            assert!(
                error.message().contains("Permission check failed"),
                "unexpected error for {stmt}: {}",
                error
            );
            let stream_error = match service.execute_stream(guest, stmt).await {
                Ok(_) => panic!("guest must not pass the streaming gate for {stmt}"),
                Err(error) => error,
            };
            assert!(
                stream_error.message().contains("Permission check failed"),
                "unexpected streaming error for {stmt}: {}",
                stream_error
            );
        }
    }

    #[tokio::test]
    async fn user_listing_reads_require_admin_or_self() {
        use super::context::QueryExecutionContext;

        let fixture = MaintenanceTestService::new().await;
        let service = &fixture.service;
        let admin = fixture
            .session_with_role("read_admin", linkrs_core::RoleType::Admin)
            .await;
        let _user = fixture
            .session_with_role("read_user", linkrs_core::RoleType::User)
            .await;
        let guest = fixture
            .session_with_role("read_guest", linkrs_core::RoleType::Guest)
            .await;

        fn gate(
            service: &GraphService<crate::storage::GraphStorage>,
            username: &str,
            space_id: i64,
            stmt: &str,
        ) -> Result<(), super::GraphServiceError> {
            let context = QueryExecutionContext {
                stmt,
                parsed_ast: None,
                space_id,
                parameters: None,
                session_variables: None,
            };
            service.check_query_permission(username, &context)
        }

        // SHOW USERS / SHOW ROLES: Admin passes, User and Guest are rejected.
        gate(service, "read_admin", fixture.space_id, "SHOW USERS").expect("admin lists users");
        gate(service, "read_admin", fixture.space_id, "SHOW ROLES IN maint_space")
            .expect("admin lists roles");
        for stmt in ["SHOW USERS", "SHOW ROLES", "SHOW ROLES IN maint_space"] {
            for username in ["read_user", "read_guest"] {
                let error = gate(service, username, fixture.space_id, stmt)
                    .expect_err("non-admin must not list users/roles");
                assert!(
                    error.message().contains("only Admin or God"),
                    "unexpected error for {username} {stmt}: {}",
                    error.message()
                );
            }
        }

        // DESCRIBE USER: self always passes, others require Admin.
        for username in ["read_admin", "read_user", "read_guest"] {
            gate(
                service,
                username,
                fixture.space_id,
                &format!("DESCRIBE USER {username}"),
            )
            .unwrap_or_else(|_| panic!("{username} must describe self"));
        }
        for username in ["read_user", "read_guest"] {
            let error = gate(service, username, fixture.space_id, "DESCRIBE USER someone_else")
                .expect_err("non-admin must not describe others");
            assert!(
                error.message().contains("only Admin or God"),
                "unexpected error: {}",
                error.message()
            );
        }

        // Both entries enforce the same gate.
        let error = service
            .execute(guest, "SHOW USERS")
            .await
            .expect_err("guest must not list users");
        assert!(
            error.message().contains("Permission check failed"),
            "unexpected error: {}",
            error.message()
        );
        service
            .execute(admin, "SHOW USERS")
            .await
            .expect("admin lists users");
        service
            .execute_stream(admin, "SHOW ROLES")
            .await
            .expect("admin lists roles on the streaming entry");

        // Self-description runs end to end once the account exists.
        service
            .execute(admin, "CREATE USER self_user WITH PASSWORD 'SelfPass123'")
            .await
            .expect("admin creates the account");
        let flown = service
            .get_session_manager()
            .create_session("self_user".to_string(), "127.0.0.1".to_string())
            .await
            .expect("create session");
        flown.set_space(fixture.space.clone());
        service
            .execute(flown.id(), "DESCRIBE USER self_user")
            .await
            .expect("self-description succeeds");
        let denied = service
            .execute(guest, "DESCRIBE USER self_user")
            .await
            .expect_err("guest must not describe others");
        assert!(
            denied.message().contains("Permission check failed"),
            "unexpected error: {}",
            denied.message()
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

    #[tokio::test]
    async fn use_statement_binds_session_space_from_parsed_form() {
        let fixture = MaintenanceTestService::new().await;
        let session = fixture
            .session_with_role("use_user", linkrs_core::RoleType::User)
            .await;

        // Any whitespace separates `USE` from the space name, so the binding
        // must follow the parsed statement rather than a literal text prefix.
        for stmt in ["USE maint_space", "USE\tmaint_space", "USE\nmaint_space"] {
            fixture
                .service
                .execute(session, stmt)
                .await
                .expect("space switch must execute");
            let bound = fixture
                .service
                .get_session_manager()
                .find_session(session)
                .expect("session must exist")
                .space()
                .map(|space| space.id);
            assert_eq!(
                bound,
                Some(fixture.space_id as u64),
                "session space must bind for {stmt:?}",
            );
        }
    }

    #[tokio::test]
    async fn use_statement_binds_session_space_on_the_streaming_entry() {
        let fixture = MaintenanceTestService::new().await;
        let session = fixture
            .session_with_role("stream_use_user", linkrs_core::RoleType::User)
            .await;

        // The streaming entry snapshots the session space before executing, so a
        // space switch must be routed to the materialized path that binds it.
        fixture
            .service
            .execute_stream(session, "USE\tmaint_space")
            .await
            .expect("streaming space switch must execute");
        let bound = fixture
            .service
            .get_session_manager()
            .find_session(session)
            .expect("session must exist")
            .space()
            .map(|space| space.id);
        assert_eq!(
            bound,
            Some(fixture.space_id as u64),
            "streaming space switch must bind the session space",
        );
    }

    #[tokio::test]
    async fn session_inspection_serves_live_registries() {
        let fixture = MaintenanceTestService::new().await;
        let service = &fixture.service;
        let admin = fixture
            .session_with_role("inspect_admin", linkrs_core::RoleType::Admin)
            .await;
        let user = fixture
            .session_with_role("inspect_user", linkrs_core::RoleType::User)
            .await;

        // SHOW SESSIONS requires Admin and lists the caller's own row.
        let sessions = service
            .execute(admin, "SHOW SESSIONS")
            .await
            .expect("admin lists sessions");
        assert!(sessions
            .columns()
            .contains(&"session_id".to_string()));
        assert!(
            sessions.rows().iter().any(|row| row.first()
                .is_some_and(|value| *value == linkrs_core::Value::BigInt(admin))),
            "own session row must be present"
        );
        let denied = service
            .execute(user, "SHOW SESSIONS")
            .await
            .expect_err("user must not list sessions");
        assert!(
            denied.message().contains("Permission check failed"),
            "unexpected error: {}",
            denied.message()
        );

        // SHOW QUERIES is readable and empty while nothing runs.
        let queries = service
            .execute(user, "SHOW QUERIES")
            .await
            .expect("user lists queries");
        assert!(queries.columns().contains(&"query_id".to_string()));
        assert!(queries.rows().is_empty());

        // A tracked stand-in is killable through the statement path; the
        // second attempt reports the non-running state.
        service
            .query_manager()
            .register_query_with_id(
                424201,
                admin,
                "inspect_admin".to_string(),
                None,
                "MATCH (n) RETURN n".to_string(),
            )
            .expect("register tracked query");
        let killed = service
            .execute(admin, &format!("KILL QUERY {admin}, 424201"))
            .await
            .expect("kill tracked query");
        assert_eq!(
            killed.columns(),
            &[
                "session_id".to_string(),
                "query_id".to_string(),
                "status".to_string()
            ]
        );
        assert_eq!(killed.rows().len(), 1);
        let repeat = service
            .execute(admin, &format!("KILL QUERY {admin}, 424201"))
            .await
            .expect_err("repeat kill must fail");
        assert!(
            repeat.message().contains("not running"),
            "unexpected error: {}",
            repeat.message()
        );
        let foreign = service
            .execute(admin, "KILL QUERY 999999, 424201")
            .await
            .expect_err("unknown session must fail");
        assert!(
            foreign.message().contains("does not exist"),
            "unexpected error: {}",
            foreign.message()
        );

        // The streaming entry reroutes inspection to the same path.
        service
            .execute_stream(admin, "SHOW SESSIONS")
            .await
            .expect("streaming inspection must execute");
    }

    #[test]
    fn write_and_schema_statements_map_above_read() {
        use crate::graph_service::GraphService as TestService;
        use crate::query::parser::ast::StmtCategory;
        use crate::storage::MockStorage;

        fn classified(text: &str) -> (Option<linkrs_core::Permission>, StmtCategory) {
            let stmt = TestService::<MockStorage>::parse_stmt(text)
                .unwrap_or_else(|| panic!("statement should parse: {text}"));
            let category = stmt.category();
            let permission = TestService::<MockStorage>::permission_for_stmt(&stmt);
            (permission, category)
        }

        // DML writes never fall through to the read default: UPDATE and UPSERT
        // share the Update variant (`is_upsert` only changes the write mode).
        for (text, kind) in [
            ("UPDATE 1 ON Person SET age = 2", "UPDATE"),
            (
                "UPSERT VERTEX ON Person SET name = 'Alice' WHERE id(vid) == 1",
                "UPSERT",
            ),
            ("MERGE (v:Person {name: 'Alice'}) SET v.age = 30", "MERGE"),
        ] {
            let (permission, category) = classified(text);
            assert_eq!(
                permission,
                Some(linkrs_core::Permission::Write),
                "{kind} must require Write"
            );
            assert_eq!(category, StmtCategory::Dml, "{kind} must classify as DML");
            let stmt = TestService::<MockStorage>::parse_stmt(text).expect("parses");
            assert_eq!(stmt.kind(), kind);
        }

        // Schema objects require Schema (USER cannot create them); spaces stay Admin.
        for (text, expected) in [
            ("CREATE TAG t (id: INT)", linkrs_core::Permission::Schema),
            (
                "CREATE INDEX idx ON Person (name)",
                linkrs_core::Permission::Schema,
            ),
            ("CREATE SPACE s", linkrs_core::Permission::Admin),
            ("DROP SPACE s", linkrs_core::Permission::Admin),
        ] {
            let (permission, category) = classified(text);
            assert_eq!(permission, Some(expected), "{text} misclassified");
            assert_eq!(category, StmtCategory::Ddl, "{text} must classify as DDL");
        }
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
            "UPDATE CONFIGS database max_sessions = 100"
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
