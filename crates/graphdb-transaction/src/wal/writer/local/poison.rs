//! Local WAL writer - poison module

use std::sync::atomic::Ordering;

use graphdb_core::wal::types::{WalError, WalResult};

use super::LocalWalWriter;

impl LocalWalWriter {
    /// Check if the WAL is poisoned.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst)
    }

    /// Get the poison reason, if any.
    pub fn poison_reason(&self) -> Option<String> {
        self.poison_reason.lock().ok()?.clone()
    }

    /// Poison the WAL writer. All subsequent write operations will fail with WalError::Poisoned.
    pub fn poison(&self, reason: String) {
        if self
            .poisoned
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            if let Ok(mut guard) = self.poison_reason.lock() {
                *guard = Some(reason.clone());
            }
            log::error!("WAL poisoned: {}", reason);
        }
    }

    pub(crate) fn check_poisoned(&self) -> WalResult<()> {
        if self.poisoned.load(Ordering::SeqCst) {
            let reason = self
                .poison_reason
                .lock()
                .ok()
                .and_then(|g| (*g).clone())
                .unwrap_or_else(|| "Unknown reason".to_string());
            Err(WalError::Poisoned(reason))
        } else {
            Ok(())
        }
    }

}
