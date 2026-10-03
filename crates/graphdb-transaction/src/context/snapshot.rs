//! Transaction context behavior: snapshot isolation timestamps and serializable read tracking

use std::collections::HashSet;

use super::TransactionContext;
use crate::wal::Timestamp;
impl TransactionContext {
    /// Get the effective read timestamp for reads
    pub fn effective_read_timestamp(&self) -> Timestamp {
        self.refreshed_read_ts
            .read()
            .map(|refreshed| refreshed.max(self.start_timestamp))
            .unwrap_or(self.start_timestamp)
    }

    /// Refresh the ReadCommitted statement snapshot
    pub fn set_refreshed_read_ts(&self, ts: Timestamp) {
        *self.refreshed_read_ts.write() = Some(ts);
    }

    /// Install a new statement snapshot pin, returning the previous one.
    ///
    /// The manager registers the returned value's release with the global
    /// snapshot tracker; tracking the pin inside the context keeps
    /// replace-on-refresh and take-on-finish atomic from the outside.
    pub fn set_statement_snapshot_pin(&self, ts: Timestamp) -> Option<Timestamp> {
        self.statement_snapshot_pin.write().replace(ts)
    }

    /// Take the current statement snapshot pin, if any.
    pub fn take_statement_snapshot_pin(&self) -> Option<Timestamp> {
        self.statement_snapshot_pin.write().take()
    }

    pub fn serializable_full_scan_threshold(&self) -> Option<usize> {
        self.serializable_full_scan_threshold
    }

    /// Record a resource read for SSI rw-dependency tracking.
    pub fn record_ssi_read(&self, resource: crate::types::ResourceId) {
        self.ssi_state.write().record_read(resource);
    }

    /// Record a resource write for SSI rw-dependency tracking.
    pub fn record_ssi_write(&self, resource: crate::types::ResourceId) {
        self.ssi_state.write().record_write(resource);
    }

    /// Get the set of resources read by this transaction (for SSI).
    pub fn get_ssi_read_resources(&self) -> HashSet<crate::types::ResourceId> {
        self.ssi_state.read().read_resources().clone()
    }

    /// Get the set of resources written by this transaction (for SSI).
    pub fn get_ssi_write_resources(&self) -> HashSet<crate::types::ResourceId> {
        self.ssi_state.read().write_resources().clone()
    }

    /// Clear SSI read locks (called on commit/abort).
    pub fn clear_ssi_state(&self) {
        let mut state = self.ssi_state.write();
        *state = crate::types::SsiState::new();
    }
}
