//! Network Session Management Module
//!
//! Providing lifecycle management for network connection sessions

pub mod error;
pub mod request_context;
pub mod session_manager;

pub use crate::api::server::client::{ClientSession, Session};
pub use error::{SessionError, SessionResult};
pub use request_context::{build_query_request_context, RequestContext};
pub use session_manager::{GraphSessionManager, SessionInfo, DEFAULT_SESSION_IDLE_TIMEOUT};
