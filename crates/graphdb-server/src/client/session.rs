use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: i64,
    pub user_name: String,
    pub space_name: Option<String>,
    pub graph_addr: Option<String>,
    pub timezone: Option<i32>,
}

#[derive(Debug)]
pub struct SessionContext {
    session: Arc<RwLock<Session>>,
}

impl SessionContext {
    pub fn new(session: Session) -> Self {
        Self {
            session: Arc::new(RwLock::new(session)),
        }
    }

    pub fn id(&self) -> i64 {
        self.session.read().session_id
    }

    pub fn user(&self) -> String {
        self.session.read().user_name.clone()
    }

    pub fn space_name(&self) -> Option<String> {
        self.session.read().space_name.clone()
    }

    pub fn update_space_name(&self, space_name: String) {
        self.session.write().space_name = Some(space_name);
    }

    pub fn timezone(&self) -> Option<i32> {
        self.session.read().timezone
    }

    pub fn set_timezone(&self, timezone: i32) {
        self.session.write().timezone = Some(timezone);
    }

    pub fn graph_addr(&self) -> Option<String> {
        self.session.read().graph_addr.clone()
    }

    pub fn update_graph_addr(&self, host_addr: String) {
        self.session.write().graph_addr = Some(host_addr);
    }

    pub fn get_session(&self) -> Session {
        self.session.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_context_creation() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let context = SessionContext::new(session);
        assert_eq!(context.id(), 123);
        assert_eq!(context.user(), "testuser");
    }

    #[test]
    fn test_session_context_timezone() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let context = SessionContext::new(session);
        assert!(context.timezone().is_none());

        context.set_timezone(8);
        assert_eq!(context.timezone(), Some(8));
    }

    #[test]
    fn test_session_context_graph_addr() {
        let session = Session {
            session_id: 123,
            user_name: "testuser".to_string(),
            space_name: None,
            graph_addr: None,
            timezone: None,
        };

        let context = SessionContext::new(session);
        assert!(context.graph_addr().is_none());

        context.update_graph_addr("127.0.0.1:9779".to_string());
        assert_eq!(context.graph_addr(), Some("127.0.0.1:9779".to_string()));
    }
}
