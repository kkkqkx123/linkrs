//! Online migration drain fence shared by embedded and server callers.
//!
//! Bridges `graphdb_transaction::issue_request_drain` into the migration
//! engine's [`graphdb_migration::SchemaWriteFence`] hook so schema switches
//! run inside a bounded write stall while the data backfill stays online.

use std::sync::Arc;
use std::time::Duration;

use graphdb_core::event_dispatch::EventSubscriptions;

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

/// Single online execution entry shared by embedded, HTTP and gRPC callers.
///
/// Holds a drain fence only across schema-modifying steps (data-only plans
/// skip it entirely), runs the plan, and confirms the switch on success.
/// Concurrency is guarded by the engine's own per-target lock; no separate
/// begin guard is needed here. Metric recording and event-registry setup
/// stay with the callers, which own their respective state sources.
pub fn execute_online_migration<S>(
    storage: &mut S,
    plan: &graphdb_migration::MigrationPlan,
    config: &graphdb_migration::MigrationConfig,
    gate: &Arc<graphdb_transaction::CheckpointGate>,
    event_registry: Option<&Arc<EventSubscriptions<graphdb_migration::MigrationEvent>>>,
) -> Result<graphdb_migration::MigrationReport, graphdb_migration::MigrationError>
where
    S: graphdb_storage::StorageReader
        + graphdb_storage::StorageWriter
        + graphdb_storage::StorageSchemaOps
        + graphdb_storage::AutoCommitGroupOps
        + graphdb_storage::AutoCommitBatchOps
        + ?Sized,
{
    let fence = if plan.has_schema_modifying_steps() {
        Some(CheckpointGateSchemaFence::new(
            Arc::clone(gate),
            Duration::from_millis(config.drain_timeout_ms.max(1)),
        ))
    } else {
        None
    };
    let report = graphdb_migration::execute_migration_plan_with_options(
        storage,
        plan,
        graphdb_migration::ExecuteOptions {
            config: Some(config),
            event_registry,
            schema_fence: fence
                .as_ref()
                .map(|f| f as &dyn graphdb_migration::SchemaWriteFence),
            ..Default::default()
        },
    )?;
    if report.success {
        let (_, audit) =
            graphdb_transaction::maintenance::issue_confirm_switch(plan.version_range.to);
        log::info!("{audit}");
    }
    Ok(report)
}
