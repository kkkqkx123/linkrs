//! Session management module
//!
//! Provides session lifecycle management for both HTTP and embedded connections.

pub mod manager;
pub mod variables;

// Re-export main types
pub use manager::{Session, SessionManager};
pub use variables::VariableStore;
