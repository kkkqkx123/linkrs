//! Online migration maintenance commands.
//!
//! Transaction-layer side of the online redistribution protocol. The engine
//! owns directories, manifests, and generations; this module only issues the
//! three maintenance commands and holds the drain fence. It never touches
//! shard directories.

use std::sync::Arc;
use std::time::Duration;

use super::checkpoint::CheckpointGate;
use super::error::TransactionError;

/// Maintenance commands the transaction layer issues to the engine driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationCommand {
    /// Open a migration: refuse when a schema change is in flight.
    BeginMigration,
    /// Briefly drain in-flight writes for the tail redo plus adopt gate.
    RequestDrain,
    /// Audit-only confirmation after the engine swaps directories.
    ConfirmSwitch,
}

/// Drain fence held across the bounded switch window. New writes are
/// refused while held; dropping or completing the fence releases them.
/// Overruns surface as checkpoint timeouts so the engine rolls back
/// instead of stretching the write stall.
pub struct MigrationDrainFence {
    gate: Arc<CheckpointGate>,
    released: bool,
}

impl MigrationDrainFence {
    fn hold(gate: &Arc<CheckpointGate>, timeout: Duration) -> Result<Self, TransactionError> {
        gate.pause_writes_and_drain(timeout)?;
        Ok(Self {
            gate: Arc::clone(gate),
            released: false,
        })
    }

    /// Complete the drain and release new writes.
    pub fn complete(mut self) {
        self.released = true;
        self.gate.resume_writes();
    }
}

impl Drop for MigrationDrainFence {
    fn drop(&mut self) {
        if !self.released {
            self.gate.resume_writes();
        }
    }
}

/// Issue [`MigrationCommand::BeginMigration`]: refuse the migration when a
/// schema change is in flight or a migration fence is already held.
pub fn issue_begin_migration(
    has_pending_schema_change: bool,
    migration_active: bool,
) -> Result<MigrationCommand, TransactionError> {
    if has_pending_schema_change {
        return Err(TransactionError::begin_failed(
            "migration refused: a schema change is in flight; finish or abort it first",
        ));
    }
    if migration_active {
        return Err(TransactionError::too_many_transactions());
    }
    Ok(MigrationCommand::BeginMigration)
}

/// Issue [`MigrationCommand::RequestDrain`]: briefly refuse new writes and
/// drain in-flight ones. A timeout error means the engine must roll back,
/// never stretch the stall.
pub fn issue_request_drain(
    gate: &Arc<CheckpointGate>,
    timeout: Duration,
) -> Result<MigrationDrainFence, TransactionError> {
    MigrationDrainFence::hold(gate, timeout)
}

/// Issue [`MigrationCommand::ConfirmSwitch`]: audit-only, no directory work.
pub fn issue_confirm_switch(target_generation: u64) -> (MigrationCommand, String) {
    (
        MigrationCommand::ConfirmSwitch,
        format!("migration confirmed switch to generation {target_generation}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_begin_migration_refuses_schema_change() {
        assert!(issue_begin_migration(true, false).is_err());
    }

    #[test]
    fn test_begin_migration_refuses_concurrent_session() {
        assert!(issue_begin_migration(false, true).is_err());
        assert_eq!(
            issue_begin_migration(false, false).expect("begin"),
            MigrationCommand::BeginMigration
        );
    }

    #[test]
    fn test_request_drain_holds_and_releases() {
        let gate = Arc::new(CheckpointGate::new());
        {
            let fence = issue_request_drain(&gate, Duration::from_secs(5)).expect("drain");
            assert!(gate.is_paused());
            fence.complete();
        }
        assert!(!gate.is_paused());
    }

    #[test]
    fn test_drain_timeout_means_rollback() {
        let gate = Arc::new(CheckpointGate::new());
        gate.acquire_write().expect("slot");
        let err = match issue_request_drain(&gate, Duration::from_millis(20)) {
            Ok(fence) => {
                fence.complete();
                panic!("overrun drain must time out");
            }
            Err(err) => err,
        };
        assert_eq!(
            err.kind(),
            crate::error::TransactionErrorKind::CheckpointTimeout
        );
        assert!(!gate.is_paused());
        gate.release_write();
    }

    #[test]
    fn test_confirm_switch_is_audit_only() {
        let (command, audit) = issue_confirm_switch(6);
        assert_eq!(command, MigrationCommand::ConfirmSwitch);
        assert!(audit.contains('6'));
    }
}
