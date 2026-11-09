//! Cross-platform service integration facade.
//!
//! Every platform reports the same two lifecycle events; the implementation
//! behind them is OS-specific. Callers use this module so no call site needs
//! platform conditionals.

#[cfg(not(windows))]
pub use crate::systemd::{notify_ready, notify_stopping};
#[cfg(windows)]
pub use crate::windows_service::{notify_ready, notify_stopping};
