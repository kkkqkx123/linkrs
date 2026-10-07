//! Online migration drain fence shared by embedded and server callers.
//!
//! Bridges `graphdb_transaction::issue_request_drain` into the migration
//! engine's [`graphdb_migration::SchemaWriteFence`] hook so schema switches
//! run inside a bounded write stall while the data backfill stays online.

use std::sync::Arc;
use std::time::Duration;

/// [`graphdb_migration::SchemaWriteFence`] backed by the transaction
/// checkpoint gate.
pub struct CheckpointGateSchemaFence {
    gate: Arc<graphdb_transaction::CheckpointGate>,
    timeout: Duration,
    held: parking_lot::Mutex<Option<graphdb_transaction::MigrationDrainFence>>,
}

impl CheckpointGateSchemaFence {
    pub fn new(gate: Arc<graphdb_transaction::CheckpointGate>, timeout: Duration) -> Self {
        Self {
            gate,
            timeout,
            held: parking_lot::Mutex::new(None),
        }
    }
}

impl graphdb_migration::SchemaWriteFence for CheckpointGateSchemaFence {
    fn hold(&self) -> Result<(), graphdb_migration::MigrationError> {
        let fence = graphdb_transaction::maintenance::issue_request_drain(&self.gate, self.timeout)
            .map_err(|e| graphdb_migration::MigrationError::Lock(e.to_string()))?;
        *self.held.lock() = Some(fence);
        Ok(())
    }

    fn release(&self) {
        if let Some(fence) = self.held.lock().take() {
            fence.complete();
        }
    }
}
