use std::sync::Arc;

/// Session and query lifecycle notification.
///
/// `SessionCreated` / `SessionDestroyed` are emitted by the server-side
/// session manager, while `QueryStarted` / `QueryCompleted` /
/// `SlowQueryDetected` are emitted by `QueryManager`. A single enum is used
/// so observers can subscribe once and receive both halves.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    SessionCreated {
        session_id: i64,
        user_name: String,
    },
    SessionDestroyed {
        session_id: i64,
    },
    QueryStarted {
        session_id: i64,
        query_id: i64,
        query_text: String,
    },
    QueryCompleted {
        session_id: i64,
        query_id: i64,
        duration_ms: i64,
        success: bool,
    },
    SlowQueryDetected {
        session_id: i64,
        query_id: i64,
        duration_ms: i64,
        threshold_ms: i64,
    },
}

/// Runtime observer for session/query events.
pub type SessionEventCallback = Arc<dyn Fn(&SessionEvent) + Send + Sync>;
