use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Instant;

use graphdb_query::executor::streaming::runtime::ExecutionRuntime;

use super::query_context::QueryContext;
use super::role_context::RoleContext;
use super::session::Session;
use super::space_context::SpaceContext;
use super::statistics::StatisticsContext;
use super::transaction_context::TransactionContext;
use crate::api::session_variables::SessionVariables;
use graphdb_core::error::QueryResult;
use graphdb_core::types::SpaceSummary;
use graphdb_core::Value;

#[derive(Debug)]
pub struct ClientSession {
    session_context: super::session::SessionContext,
    space_context: SpaceContext,
    role_context: RoleContext,
    query_context: QueryContext,
    transaction_context: TransactionContext,
    statistics_context: StatisticsContext,
    idle_start_time: Arc<RwLock<Instant>>,
    /// Session-scoped user variables (`$name`) with transaction overlay.
    session_variables: Arc<SessionVariables>,
}

impl ClientSession {
    pub fn new(session: Session) -> Arc<Self> {
        Arc::new(Self {
            session_context: super::session::SessionContext::new(session),
            space_context: SpaceContext::new(),
            role_context: RoleContext::new(),
            query_context: QueryContext::new(),
            transaction_context: TransactionContext::new(),
            statistics_context: StatisticsContext::new(),
            idle_start_time: Arc::new(RwLock::new(Instant::now())),
            session_variables: Arc::new(SessionVariables::new()),
        })
    }

    pub fn id(&self) -> i64 {
        self.session_context.id()
    }

    pub fn space(&self) -> Option<SpaceSummary> {
        self.space_context.space()
    }

    pub fn set_space(&self, space: SpaceSummary) {
        self.space_context.set_space(space);
    }

    pub fn clear_space(&self) {
        self.space_context.clear_space();
    }

    pub fn space_name(&self) -> Option<String> {
        self.session_context.space_name()
    }

    pub fn user(&self) -> String {
        self.session_context.user()
    }

    pub fn roles(&self) -> std::collections::HashMap<i64, graphdb_core::RoleType> {
        self.role_context.roles()
    }

    pub fn role_with_space(&self, space: i64) -> Option<graphdb_core::RoleType> {
        self.role_context.role_with_space(space)
    }

    pub fn is_god(&self) -> bool {
        self.role_context.is_god()
    }

    pub fn is_admin(&self) -> bool {
        self.role_context.is_admin()
    }

    pub fn set_role(&self, space: i64, role: graphdb_core::RoleType) {
        self.role_context.set_role(space, role);
    }

    pub fn idle_seconds(&self) -> u64 {
        self.idle_start_time.read().elapsed().as_secs()
    }

    pub fn charge(&self) {
        *self.idle_start_time.write() = Instant::now();
    }

    pub fn timezone(&self) -> Option<i32> {
        self.session_context.timezone()
    }

    pub fn set_timezone(&self, timezone: i32) {
        self.session_context.set_timezone(timezone);
    }

    pub fn graph_addr(&self) -> Option<String> {
        self.session_context.graph_addr()
    }

    pub fn update_graph_addr(&self, host_addr: String) {
        self.session_context.update_graph_addr(host_addr);
    }

    pub fn get_session(&self) -> Session {
        self.session_context.get_session()
    }

    pub fn update_space_name(&self, space_name: String) {
        self.session_context.update_space_name(space_name);
    }

    pub fn add_query(&self, ep_id: u32, query_context: String) {
        self.query_context
            .add_query(ep_id, query_context, self.id());
    }

    /// Register a streaming query with a weak runtime reference for KILL QUERY support.
    pub fn register_streaming_query(
        &self,
        query_id: u32,
        query_text: String,
        runtime: Weak<ExecutionRuntime>,
    ) {
        self.query_context
            .register_streaming_query(query_id, query_text, runtime, self.id());
    }

    /// Unregister a streaming query on completion.
    pub fn unregister_streaming_query(&self, query_id: u32) {
        self.query_context
            .unregister_streaming_query(query_id, self.id());
    }

    pub fn delete_query(&self, ep_id: u32) {
        self.query_context.delete_query(ep_id, self.id());
    }

    pub fn find_query(&self, ep_id: u32) -> bool {
        self.query_context.find_query(ep_id)
    }

    pub fn mark_query_killed(&self, ep_id: u32) {
        self.query_context.mark_query_killed(ep_id, self.id());
    }

    pub fn mark_all_queries_killed(&self) {
        self.query_context.mark_all_queries_killed(self.id());
    }

    pub fn active_queries_count(&self) -> usize {
        self.query_context.active_queries_count()
    }

    pub fn kill_query(&self, query_id: u32) -> QueryResult<()> {
        self.query_context.kill_query(query_id, self.id())
    }

    pub fn kill_multiple_queries(&self, query_ids: &[u32]) -> Vec<QueryResult<()>> {
        self.query_context
            .kill_multiple_queries(query_ids, self.id())
    }

    pub fn current_transaction(&self) -> Option<graphdb_transaction::TransactionId> {
        self.transaction_context.current_transaction()
    }

    pub fn bind_transaction(&self, txn_id: graphdb_transaction::TransactionId) {
        self.transaction_context.bind_transaction(txn_id, self.id());
    }

    pub fn unbind_transaction(&self) {
        self.transaction_context.unbind_transaction(self.id());
    }

    pub fn has_active_transaction(&self) -> bool {
        self.transaction_context.has_active_transaction()
    }

    pub fn is_auto_commit(&self) -> bool {
        self.transaction_context.is_auto_commit()
    }

    pub fn set_auto_commit(&self, auto_commit: bool) {
        self.transaction_context
            .set_auto_commit(auto_commit, self.id());
    }

    pub fn transaction_options(&self) -> graphdb_transaction::TransactionOptions {
        self.transaction_context.transaction_options()
    }

    pub fn set_transaction_options(&self, options: graphdb_transaction::TransactionOptions) {
        self.transaction_context.set_transaction_options(options);
    }

    pub fn push_savepoint(&self, savepoint_id: graphdb_transaction::SavepointId) {
        self.transaction_context
            .push_savepoint(savepoint_id, self.id());
    }

    pub fn savepoint_stack(&self) -> Vec<graphdb_transaction::SavepointId> {
        self.transaction_context.savepoint_stack()
    }

    pub fn clear_savepoints(&self) {
        self.transaction_context.clear_savepoints(self.id());
    }

    pub fn savepoint_count(&self) -> usize {
        self.transaction_context.savepoint_count()
    }

    pub fn statistics(&self) -> &graphdb_core::SessionStatistics {
        self.statistics_context.statistics()
    }

    // ── Session variables (`$name`) ───────────────────────────────────────

    /// Assign a session variable, recording a transaction overlay operation
    /// when an explicit transaction is active.
    pub fn set_variable(&self, name: String, value: Value) {
        self.session_variables
            .set_variable(name, value, self.has_active_transaction());
    }

    /// Effective value of a session variable: transaction overlay first,
    /// then the base store.
    pub fn variable_value(&self, name: &str) -> Option<Value> {
        self.session_variables.variable_value(name)
    }

    /// Snapshot of all session variables (base + overlay) for injection as
    /// query inputs.
    pub fn variables_snapshot(&self) -> HashMap<String, Value> {
        self.session_variables.variables_snapshot()
    }

    /// COMMIT: apply overlay operations to the base store and clear the
    /// overlay (the last assignment of each variable wins).
    pub fn commit_variables(&self) {
        self.session_variables.commit_variables();
    }

    /// Full ROLLBACK: restore pre-transaction values and clear the overlay.
    pub fn rollback_variables(&self) {
        self.session_variables.rollback_variables();
    }

    /// ROLLBACK TO SAVEPOINT: restore values assigned after the named
    /// savepoint and truncate the overlay at the savepoint marker.
    pub fn rollback_variables_to(&self, savepoint_name: &str) -> bool {
        self.session_variables.rollback_variables_to(savepoint_name)
    }

    /// RELEASE SAVEPOINT: drop the marker (operations stay part of the
    /// transaction; they are no longer individually rollback-able).
    pub fn release_variable_savepoint(&self, savepoint_name: &str) {
        self.session_variables
            .release_variable_savepoint(savepoint_name);
    }

    /// SAVEPOINT: record a variable-overlay boundary so ROLLBACK TO can
    /// restore assignments made after the savepoint.
    pub fn push_variable_savepoint(&self, savepoint_name: &str) {
        self.session_variables
            .push_variable_savepoint(savepoint_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::DataType;

    #[test]
    fn test_client_session_creation() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        assert_eq!(client_session.id(), 123);
        assert_eq!(client_session.user(), "testuser");
        assert_eq!(client_session.roles().len(), 0);
        assert!(!client_session.is_admin());
    }

    #[test]
    fn test_client_session_space_management() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        assert!(client_session.space().is_none());
        assert!(client_session.space_name().is_none());

        let space_info = SpaceSummary::new(456, "test_space".to_string(), DataType::BigInt);
        client_session.set_space(space_info.clone());

        assert_eq!(client_session.space().expect("space should exist").id, 456);
        assert_eq!(
            client_session.space().expect("space should exist").name,
            "test_space"
        );
        client_session.update_space_name("new_space".to_string());
        assert_eq!(
            client_session
                .space_name()
                .expect("space_name should exist"),
            "new_space"
        );

        client_session.clear_space();
        assert!(client_session.space().is_none());
    }

    #[test]
    fn test_client_session_role_management() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        assert!(client_session.role_with_space(1).is_none());
        assert!(!client_session.is_admin());
        assert!(!client_session.is_god());

        client_session.set_role(1, graphdb_core::RoleType::Admin);
        assert_eq!(
            client_session
                .role_with_space(1)
                .expect("role should exist"),
            graphdb_core::RoleType::Admin
        );
        assert!(client_session.is_admin());
        assert!(!client_session.is_god());

        client_session.set_role(2, graphdb_core::RoleType::God);
        assert!(client_session.is_god());
    }

    #[test]
    fn test_client_session_idle_time() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        let idle1 = client_session.idle_seconds();
        assert_eq!(idle1, 0);

        std::thread::sleep(std::time::Duration::from_millis(1100));
        let idle2 = client_session.idle_seconds();
        assert!(idle2 > 0);

        client_session.charge();
        let idle3 = client_session.idle_seconds();
        assert_eq!(idle3, 0);
    }

    #[test]
    fn test_client_session_query_management() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        assert_eq!(client_session.active_queries_count(), 0);
        assert!(!client_session.find_query(1));

        client_session.add_query(1, "SELECT * FROM user".to_string());
        assert_eq!(client_session.active_queries_count(), 1);
        assert!(client_session.find_query(1));

        client_session.delete_query(1);
        assert_eq!(client_session.active_queries_count(), 0);
        assert!(!client_session.find_query(1));

        client_session.add_query(2, "MATCH (n) RETURN n".to_string());
        let result = client_session.kill_query(2);
        assert!(result.is_ok());
        assert!(!client_session.find_query(2));

        let result = client_session.kill_query(999);
        assert!(result.is_err());

        client_session.add_query(3, "query 3".to_string());
        client_session.add_query(4, "query 4".to_string());
        let results = client_session.kill_multiple_queries(&[3, 4, 5]);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[2].is_err());
    }

    #[test]
    fn test_client_session_transaction_management() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        assert!(client_session.current_transaction().is_none());
        assert!(!client_session.has_active_transaction());
        assert!(client_session.is_auto_commit());

        client_session.bind_transaction(graphdb_transaction::TransactionId(1001));
        assert_eq!(
            client_session
                .current_transaction()
                .expect("current_transaction should exist"),
            graphdb_transaction::TransactionId(1001)
        );
        assert!(client_session.has_active_transaction());

        client_session.unbind_transaction();
        assert!(client_session.current_transaction().is_none());

        client_session.set_auto_commit(false);
        assert!(!client_session.is_auto_commit());

        let options = graphdb_transaction::TransactionOptions::default();
        client_session.set_transaction_options(options.clone());
        assert_eq!(client_session.transaction_options(), options);
    }

    #[test]
    fn test_client_session_savepoint_management() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        assert_eq!(client_session.savepoint_count(), 0);
        assert!(client_session.savepoint_stack().is_empty());

        client_session.push_savepoint(1);
        client_session.push_savepoint(2);
        assert_eq!(client_session.savepoint_count(), 2);
        assert_eq!(client_session.savepoint_stack(), vec![1, 2]);

        client_session.clear_savepoints();
        assert_eq!(client_session.savepoint_count(), 0);
    }

    #[test]
    fn test_session_variables_outside_transaction() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        client_session.set_variable("x".to_string(), Value::Int(1));
        assert_eq!(
            client_session.variable_value("x"),
            Some(Value::Int(1)),
            "set outside a transaction writes the base store"
        );

        // Overwrite.
        client_session.set_variable("x".to_string(), Value::Int(2));
        assert_eq!(client_session.variable_value("x"), Some(Value::Int(2)));

        let snapshot = client_session.variables_snapshot();
        assert_eq!(snapshot.get("x"), Some(&Value::Int(2)));
    }

    #[test]
    fn test_session_variables_transaction_rollback_restores() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        // Pre-transaction value.
        client_session.set_variable("x".to_string(), Value::Int(1));

        client_session.bind_transaction(graphdb_transaction::TransactionId(7));
        assert!(client_session.has_active_transaction());

        // Assignments inside the transaction go to the overlay.
        client_session.set_variable("x".to_string(), Value::Int(100));
        client_session.set_variable("y".to_string(), Value::string("txn"));
        assert_eq!(client_session.variable_value("x"), Some(Value::Int(100)));
        assert_eq!(
            client_session.variable_value("y"),
            Some(Value::string("txn"))
        );

        // Full rollback restores pre-transaction values.
        client_session.rollback_variables();
        client_session.unbind_transaction();
        assert_eq!(client_session.variable_value("x"), Some(Value::Int(1)));
        assert_eq!(client_session.variable_value("y"), None);
    }

    #[test]
    fn test_session_variables_transaction_commit_merges() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        client_session.bind_transaction(graphdb_transaction::TransactionId(8));
        client_session.set_variable("a".to_string(), Value::Int(5));

        client_session.commit_variables();
        client_session.unbind_transaction();
        assert_eq!(client_session.variable_value("a"), Some(Value::Int(5)));
    }

    #[test]
    fn test_session_variables_rollback_to_savepoint() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let client_session = ClientSession::new(session);

        client_session.bind_transaction(graphdb_transaction::TransactionId(9));

        client_session.set_variable("a".to_string(), Value::Int(1));
        client_session.push_variable_savepoint("sp1");
        client_session.set_variable("a".to_string(), Value::Int(2));
        client_session.set_variable("b".to_string(), Value::Int(3));

        assert!(
            client_session.rollback_variables_to("sp1"),
            "savepoint marker must be found"
        );
        assert_eq!(
            client_session.variable_value("a"),
            Some(Value::Int(1)),
            "assignment after the savepoint is restored"
        );
        assert_eq!(client_session.variable_value("b"), None);

        // Unknown savepoint: nothing changes.
        client_session.set_variable("b".to_string(), Value::Int(4));
        assert!(!client_session.rollback_variables_to("missing"));
        assert_eq!(client_session.variable_value("b"), Some(Value::Int(4)));

        // Released savepoints are no longer rollback targets.
        client_session.push_variable_savepoint("sp2");
        client_session.release_variable_savepoint("sp2");
        assert!(!client_session.rollback_variables_to("sp2"));
    }
}
