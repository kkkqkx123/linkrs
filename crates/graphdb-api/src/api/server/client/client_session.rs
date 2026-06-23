use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;

use super::query_context::QueryContext;
use super::role_context::RoleContext;
use super::session::Session;
use super::space_context::SpaceContext;
use super::statistics::StatisticsContext;
use super::transaction_context::TransactionContext;
use crate::core::error::QueryResult;
use crate::core::types::SpaceSummary;

#[derive(Debug)]
pub struct ClientSession {
    session_context: super::session::SessionContext,
    space_context: SpaceContext,
    role_context: RoleContext,
    query_context: QueryContext,
    transaction_context: TransactionContext,
    statistics_context: StatisticsContext,
    idle_start_time: Arc<RwLock<Instant>>,
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

    pub fn roles(&self) -> std::collections::HashMap<i64, crate::core::RoleType> {
        self.role_context.roles()
    }

    pub fn role_with_space(&self, space: i64) -> Option<crate::core::RoleType> {
        self.role_context.role_with_space(space)
    }

    pub fn is_god(&self) -> bool {
        self.role_context.is_god()
    }

    pub fn is_admin(&self) -> bool {
        self.role_context.is_admin()
    }

    pub fn set_role(&self, space: i64, role: crate::core::RoleType) {
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

    pub fn current_transaction(&self) -> Option<crate::transaction::TransactionId> {
        self.transaction_context.current_transaction()
    }

    pub fn bind_transaction(&self, txn_id: crate::transaction::TransactionId) {
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

    pub fn transaction_options(&self) -> crate::transaction::TransactionOptions {
        self.transaction_context.transaction_options()
    }

    pub fn set_transaction_options(&self, options: crate::transaction::TransactionOptions) {
        self.transaction_context.set_transaction_options(options);
    }

    pub fn push_savepoint(&self, savepoint_id: crate::transaction::SavepointId) {
        self.transaction_context
            .push_savepoint(savepoint_id, self.id());
    }

    pub fn savepoint_stack(&self) -> Vec<crate::transaction::SavepointId> {
        self.transaction_context.savepoint_stack()
    }

    pub fn clear_savepoints(&self) {
        self.transaction_context.clear_savepoints(self.id());
    }

    pub fn savepoint_count(&self) -> usize {
        self.transaction_context.savepoint_count()
    }

    pub fn statistics(&self) -> &crate::core::SessionStatistics {
        self.statistics_context.statistics()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::DataType;

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

        client_session.set_role(1, crate::core::RoleType::Admin);
        assert_eq!(
            client_session
                .role_with_space(1)
                .expect("role should exist"),
            crate::core::RoleType::Admin
        );
        assert!(client_session.is_admin());
        assert!(!client_session.is_god());

        client_session.set_role(2, crate::core::RoleType::God);
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

        client_session.bind_transaction(crate::transaction::TransactionId(1001));
        assert_eq!(
            client_session
                .current_transaction()
                .expect("current_transaction should exist"),
            crate::transaction::TransactionId(1001)
        );
        assert!(client_session.has_active_transaction());

        client_session.unbind_transaction();
        assert!(client_session.current_transaction().is_none());

        client_session.set_auto_commit(false);
        assert!(!client_session.is_auto_commit());

        let options = crate::transaction::TransactionOptions::default();
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
}
